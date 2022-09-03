use clap::Parser;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// The path to the application's il2cpp shared object file (libil2cpp.so)
    shared_object: String,
    /// The path to the application's il2cpp metadata file (global-metadata.dat)
    metadata: String,
    /// The path to the xref data created by xref_gen
    xref_data: String,
}

fn main() {
    let args = Args::parse();
    dbg!(args);
}
