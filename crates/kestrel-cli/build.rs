include!("../../build-support/embed_icon.rs");

fn main() {
    println!("cargo:rerun-if-changed=../../build-support/embed_icon.rs");
    embed_icon(
        Path::new("../../assets/kestrel.ico"),
        "kestrel",
        "Kestrel — autonomous coding and work agent",
    );
}
