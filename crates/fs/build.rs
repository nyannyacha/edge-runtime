use std::path::Path;

fn main() {
    let path = Path::new("./tests/.env");
    if !path.exists() {
        std::fs::File::create(path).unwrap();
    }
    println!("cargo::rerun-if-changed=tests/.env");
    if Path::new("./tests/.env").exists() {
        println!("cargo:rustc-cfg=dotenv")
    }
}
