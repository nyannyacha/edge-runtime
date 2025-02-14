use std::sync::Arc;

use deno::npm::CliNpmResolver;

use crate::virtual_fs::FileBackedVfs;

mod r#impl;
mod rt;

pub use r#impl::deno_compile_fs;
pub use r#impl::prefix_fs;
pub use r#impl::s3_fs;
pub use r#impl::static_fs;
pub use r#impl::tmp_fs;
pub use r#impl::virtual_fs;

pub struct VfsOpts {
  pub npm_resolver: Arc<dyn CliNpmResolver>,
}
