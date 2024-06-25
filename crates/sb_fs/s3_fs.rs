use std::{
    borrow::Cow,
    path::{Path, PathBuf},
    rc::Rc,
};

use aws_config::SdkConfig;
use aws_sdk_s3::Client;
use deno_fs::{AccessCheckCb, FsDirEntry, FsFileType, OpenOptions};
use deno_io::fs::{File, FsError, FsResult, FsStat};

#[derive(Debug, Clone)]
pub struct S3Fs {
    bucket_name: Cow<'static, str>,
    client: Client,
}

impl S3Fs {
    pub fn new(bucket_name: Cow<'static, str>, sdk_config: &SdkConfig) -> Self {
        Self {
            bucket_name,
            client: Client::new(sdk_config),
        }
    }

    pub fn bucket_name(&self) -> &str {
        &self.bucket_name
    }
}

#[async_trait::async_trait(?Send)]
impl deno_fs::FileSystem for S3Fs {
    fn cwd(&self) -> FsResult<PathBuf> {
        Ok(PathBuf::new())
    }

    fn tmp_dir(&self) -> FsResult<PathBuf> {
        // NOTE: Meaningless
        Err(FsError::NotSupported)
    }

    fn chdir(&self, _path: &Path) -> FsResult<()> {
        // NOTE: Meaningless
        Err(FsError::NotSupported)
    }

    fn umask(&self, _mask: Option<u32>) -> FsResult<u32> {
        // NOTE: Need declare custom metadata
        Err(FsError::NotSupported)
    }

    fn open_sync(
        &self,
        _path: &Path,
        _options: OpenOptions,
        _access_check: Option<AccessCheckCb>,
    ) -> FsResult<Rc<dyn File>> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn open_async<'a>(
        &'a self,
        _path: PathBuf,
        _options: OpenOptions,
        _access_check: Option<AccessCheckCb<'a>>,
    ) -> FsResult<Rc<dyn File>> {
        todo!()
    }

    fn mkdir_sync(&self, _path: &Path, _recursive: bool, _mode: u32) -> FsResult<()> {
        // NOTE: S3 is object key-based storage. It does not support the
        // creation of folders.
        Ok(())
    }

    async fn mkdir_async(&self, _path: PathBuf, _recursive: bool, _mode: u32) -> FsResult<()> {
        // NOTE: S3 is object key-based storage. It does not support the
        // creation of folders.
        Ok(())
    }

    fn chmod_sync(&self, _path: &Path, _mode: u32) -> FsResult<()> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn chmod_async(&self, _path: PathBuf, _mode: u32) -> FsResult<()> {
        // NOTE: Need declare custom metadata
        Err(FsError::NotSupported)
    }

    fn chown_sync(&self, _path: &Path, _uid: Option<u32>, _gid: Option<u32>) -> FsResult<()> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn chown_async(
        &self,
        _path: PathBuf,
        _uid: Option<u32>,
        _gid: Option<u32>,
    ) -> FsResult<()> {
        // NOTE: Need declare custom metadata
        Err(FsError::NotSupported)
    }

    fn remove_sync(&self, _path: &Path, _recursive: bool) -> FsResult<()> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn remove_async(&self, _path: PathBuf, _recursive: bool) -> FsResult<()> {
        todo!()
    }

    fn copy_file_sync(&self, _oldpath: &Path, _newpath: &Path) -> FsResult<()> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn copy_file_async(&self, _oldpath: PathBuf, _newpath: PathBuf) -> FsResult<()> {
        todo!()
    }

    fn cp_sync(&self, _path: &Path, _new_path: &Path) -> FsResult<()> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn cp_async(&self, _path: PathBuf, _new_path: PathBuf) -> FsResult<()> {
        todo!()
    }

    fn stat_sync(&self, _path: &Path) -> FsResult<FsStat> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn stat_async(&self, _path: PathBuf) -> FsResult<FsStat> {
        todo!()
    }

    fn lstat_sync(&self, _path: &Path) -> FsResult<FsStat> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn lstat_async(&self, _path: PathBuf) -> FsResult<FsStat> {
        // NOTE: Need declare custom metadata
        todo!()
    }

    fn realpath_sync(&self, _path: &Path) -> FsResult<PathBuf> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn realpath_async(&self, _path: PathBuf) -> FsResult<PathBuf> {
        // NOTE: Meaningless
        Err(FsError::NotSupported)
    }

    fn read_dir_sync(&self, _path: &Path) -> FsResult<Vec<FsDirEntry>> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn read_dir_async(&self, path: PathBuf) -> FsResult<Vec<FsDirEntry>> {
        let path = path.to_string_lossy();
        let has_trailing_slash = path.ends_with('/');

        let builder = self
            .client
            .list_objects_v2()
            .set_bucket(Some(self.bucket_name().into()))
            .set_delimiter(Some("/".into()))
            .set_prefix((!has_trailing_slash).then(|| "/".into()));

        let entries = vec![];
        let mut stream = builder.into_paginator().send();

        while let Some(resp) = stream.next().await {
            match resp {
                Ok(_v) => {
                    todo!();
                }

                Err(_err) => {
                    todo!();
                }
            }
        }

        Ok(entries)
    }

    fn rename_sync(&self, _oldpath: &Path, _newpath: &Path) -> FsResult<()> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn rename_async(&self, _oldpath: PathBuf, _newpath: PathBuf) -> FsResult<()> {
        // NOTE: Inefficient (Non atomic, consistency)
        Err(FsError::NotSupported)
    }

    fn link_sync(&self, _oldpath: &Path, _newpath: &Path) -> FsResult<()> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn link_async(&self, _oldpath: PathBuf, _newpath: PathBuf) -> FsResult<()> {
        // NOTE: Need declare custom metadata
        Err(FsError::NotSupported)
    }

    fn symlink_sync(
        &self,
        _oldpath: &Path,
        _newpath: &Path,
        _file_type: Option<FsFileType>,
    ) -> FsResult<()> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn symlink_async(
        &self,
        _oldpath: PathBuf,
        _newpath: PathBuf,
        _file_type: Option<FsFileType>,
    ) -> FsResult<()> {
        // NOTE: Need declare custom metadata
        Err(FsError::NotSupported)
    }

    fn read_link_sync(&self, _path: &Path) -> FsResult<PathBuf> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn read_link_async(&self, _path: PathBuf) -> FsResult<PathBuf> {
        // NOTE: Need declare custom metadata
        Err(FsError::NotSupported)
    }

    fn truncate_sync(&self, _path: &Path, _len: u64) -> FsResult<()> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn truncate_async(&self, _path: PathBuf, _len: u64) -> FsResult<()> {
        // NOTE: Inefficient (PUT)
        Err(FsError::NotSupported)
    }

    fn utime_sync(
        &self,
        _path: &Path,
        _atime_secs: i64,
        _atime_nanos: u32,
        _mtime_secs: i64,
        _mtime_nanos: u32,
    ) -> FsResult<()> {
        // NOTE: Inefficient (Sync)
        Err(FsError::NotSupported)
    }

    async fn utime_async(
        &self,
        _path: PathBuf,
        _atime_secs: i64,
        _atime_nanos: u32,
        _mtime_secs: i64,
        _mtime_nanos: u32,
    ) -> FsResult<()> {
        // NOTE: Need declare custom metadata
        Err(FsError::NotSupported)
    }
}
