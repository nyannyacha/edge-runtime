// Copyright 2018-2024 the Deno authors. All rights reserved. MIT license.

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::Path;
use std::path::PathBuf;

use base64::prelude::BASE64_STANDARD;
use base64::Engine;
use deno_core::anyhow::bail;
use deno_core::anyhow::Context;
use deno_core::error::AnyError;
use deno_npm::registry::NpmPackageVersionDistInfo;
use deno_npm::registry::NpmPackageVersionDistInfoIntegrity;
use deno_semver::package::PackageNv;
use flate2::read::GzDecoder;
use tar::Archive;
use tar::EntryType;

#[derive(Debug, Copy, Clone)]
pub enum TarballExtractionMode {
  /// Overwrites the destination directory without deleting any files.
  Overwrite,
  /// Creates and writes to a sibling temporary directory. When done, moves
  /// it to the final destination.
  ///
  /// This is more robust than `Overwrite` as it better handles multiple
  /// processes writing to the directory at the same time.
  SiblingTempDir,
}

pub fn verify_and_extract_tarball(
  package_nv: &PackageNv,
  data: &[u8],
  dist_info: &NpmPackageVersionDistInfo,
  output_folder: &Path,
  extraction_mode: TarballExtractionMode,
) -> Result<(), AnyError> {
  verify_tarball_integrity(package_nv, data, &dist_info.integrity())?;

  match extraction_mode {
    TarballExtractionMode::Overwrite => extract_tarball(data, output_folder),
    TarballExtractionMode::SiblingTempDir => {
      let temp_dir = get_atomic_dir_path(output_folder);
      extract_tarball(data, &temp_dir)?;
      rename_with_retries(&temp_dir, output_folder)
        .map_err(AnyError::from)
        .context("Failed moving extracted tarball to final destination.")
    }
  }
}

fn rename_with_retries(
  temp_dir: &Path,
  output_folder: &Path,
) -> Result<(), std::io::Error> {
  fn already_exists(err: &std::io::Error, output_folder: &Path) -> bool {
    // Windows will do an "Access is denied" error
    err.kind() == ErrorKind::AlreadyExists || output_folder.exists()
  }

  let mut count = 0;
  // renaming might be flaky if a lot of processes are trying
  // to do this, so retry a few times
  loop {
    match fs::rename(temp_dir, output_folder) {
      Ok(_) => return Ok(()),
      Err(err) if already_exists(&err, output_folder) => {
        // another process copied here, just cleanup
        let _ = fs::remove_dir_all(temp_dir);
        return Ok(());
      }
      Err(err) => {
        count += 1;
        if count > 5 {
          // too many retries, cleanup and return the error
          let _ = fs::remove_dir_all(temp_dir);
          return Err(err);
        }

        // wait a bit before retrying... this should be very rare or only
        // in error cases, so ok to sleep a bit
        let sleep_ms = std::cmp::min(100, 20 * count);
        std::thread::sleep(std::time::Duration::from_millis(sleep_ms));
      }
    }
  }
}

fn verify_tarball_integrity(
  package: &PackageNv,
  data: &[u8],
  npm_integrity: &NpmPackageVersionDistInfoIntegrity,
) -> Result<(), AnyError> {
  use ring::digest::Context;
  let (tarball_checksum, expected_checksum) = match npm_integrity {
    NpmPackageVersionDistInfoIntegrity::Integrity {
      algorithm,
      base64_hash,
    } => {
      let algo = match *algorithm {
        "sha512" => &ring::digest::SHA512,
        "sha1" => &ring::digest::SHA1_FOR_LEGACY_USE_ONLY,
        hash_kind => bail!(
          "Not implemented hash function for {}: {}",
          package,
          hash_kind
        ),
      };
      let mut hash_ctx = Context::new(algo);
      hash_ctx.update(data);
      let digest = hash_ctx.finish();
      let tarball_checksum = BASE64_STANDARD.encode(digest.as_ref());
      (tarball_checksum, base64_hash)
    }
    NpmPackageVersionDistInfoIntegrity::LegacySha1Hex(hex) => {
      let mut hash_ctx = Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
      hash_ctx.update(data);
      let digest = hash_ctx.finish();
      let tarball_checksum = faster_hex::hex_string(digest.as_ref());
      (tarball_checksum, hex)
    }
    NpmPackageVersionDistInfoIntegrity::UnknownIntegrity(integrity) => {
      bail!(
        "Not implemented integrity kind for {}: {}",
        package,
        integrity
      )
    }
  };

  if tarball_checksum != *expected_checksum {
    bail!(
      "Tarball checksum did not match what was provided by npm registry for {}.\n\nExpected: {}\nActual: {}",
      package,
      expected_checksum,
      tarball_checksum,
    )
  }
  Ok(())
}

fn extract_tarball(data: &[u8], output_folder: &Path) -> Result<(), AnyError> {
  fs::create_dir_all(output_folder)?;
  let output_folder = fs::canonicalize(output_folder)?;
  let tar = GzDecoder::new(data);
  let mut archive = Archive::new(tar);
  archive.set_overwrite(true);
  archive.set_preserve_permissions(true);
  let mut created_dirs = HashSet::new();

  for entry in archive.entries()? {
    let mut entry = entry?;
    let path = entry.path()?;
    let entry_type = entry.header().entry_type();

    // Some package tarballs contain "pax_global_header", these entries
    // should be skipped.
    if entry_type == EntryType::XGlobalHeader {
      continue;
    }

    // skip the first component which will be either "package" or the name of the package
    let relative_path = path.components().skip(1).collect::<PathBuf>();
    let absolute_path = output_folder.join(relative_path);
    let dir_path = if entry_type == EntryType::Directory {
      absolute_path.as_path()
    } else {
      absolute_path.parent().unwrap()
    };
    if created_dirs.insert(dir_path.to_path_buf()) {
      fs::create_dir_all(dir_path)?;
      let canonicalized_dir = fs::canonicalize(dir_path)?;
      if !canonicalized_dir.starts_with(&output_folder) {
        bail!(
                    "Extracted directory '{}' of npm tarball was not in output directory.",
                    canonicalized_dir.display()
                )
      }
    }

    let entry_type = entry.header().entry_type();
    match entry_type {
      EntryType::Regular => {
        entry.unpack(&absolute_path)?;
      }
      EntryType::Symlink | EntryType::Link => {
        // At the moment, npm doesn't seem to support uploading hardlinks or
        // symlinks to the npm registry. If ever adding symlink or hardlink
        // support, we will need to validate that the hardlink and symlink
        // target are within the package directory.
        log::warn!(
          "Ignoring npm tarball entry type {:?} for '{}'",
          entry_type,
          absolute_path.display()
        )
      }
      _ => {
        // ignore
      }
    }
  }
  Ok(())
}
