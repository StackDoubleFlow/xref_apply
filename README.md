# xref-apply

A command line tool to find symbol addresses in Unity IL2CPP AArch64 applications using xref traces.

## Usage

Firstly, you'll have to gather the following things:
- The traces that correspond to the specific Unity version and platform (`xref_gen.json`). You can generate the data with [xref_gen](https://github.com/StackDoubleFlow/xref_gen).
- The application's il2cpp shared object file (`libil2cpp.so`) and global metadata file (`global-metadata.dat`).

With those placed inside the `./data` directory, you can run the following command to generate a JSON file containing the addresses of symbols:
```
cargo run --release -- data/libil2cpp.so data/global-metadata.dat data/xref_gen.json
```
The output will be placed in `./data/xref_apply.json`.

## Importing into Ghidra

The ghidra script at `./data/xref_apply.py` will look for the output file `xref_apply.json` in the same directory. The script can only be ran once Auto Analysis on the binary has been completed. Once run, labels will be created for the symbols `xref_apply` had sucessfully found.
