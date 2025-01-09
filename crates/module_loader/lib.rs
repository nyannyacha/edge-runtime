use deno_core::FastString;
use deno_core::ModuleLoader;
use deno_npm::resolution::ValidSerializedNpmResolutionSnapshot;
use ext_node::NodeResolver;
use ext_node::NpmResolver;
use fs::virtual_fs::FileBackedVfs;
use fs::EszipStaticFiles;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;

pub mod metadata;
pub mod standalone;
pub mod util;

pub struct RuntimeProviders {
  pub node_resolver: Arc<NodeResolver>,
  pub npm_resolver: Arc<dyn NpmResolver>,
  pub module_loader: Rc<dyn ModuleLoader>,
  pub vfs: Arc<FileBackedVfs>,
  pub module_code: Option<FastString>,
  pub static_files: EszipStaticFiles,
  pub npm_snapshot: Option<ValidSerializedNpmResolutionSnapshot>,
  pub vfs_path: PathBuf,
}
