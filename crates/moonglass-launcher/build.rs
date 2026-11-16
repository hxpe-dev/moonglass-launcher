//! Windows: puts the icon (`assets/logo.ico`, made from `assets/logo.svg`)
//! into the executable.

fn main() {
    println!("cargo:rerun-if-changed=moonglass.rc");
    println!("cargo:rerun-if-changed=../../assets/logo.ico");
    if std::env::var_os("CARGO_CFG_WINDOWS").is_some() {
        embed_resource::compile("moonglass.rc", embed_resource::NONE)
            .manifest_required()
            .expect("embedding the icon");
    }
}
