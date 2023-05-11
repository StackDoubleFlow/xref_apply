use bad64::{Imm, Instruction, Op, Operand};
use brocolib::global_metadata::Token;
use brocolib::runtime_metadata::elf::Elf;
use brocolib::runtime_metadata::{Il2CppCodeRegistration, RuntimeMetadata};
use brocolib::Metadata;
use clap::Parser;
use color_eyre::eyre::{bail, eyre, ContextCompat, Result};
use object::{Object, ObjectSymbol};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// The path to the application's il2cpp shared object file (libil2cpp.so)
    shared_object: PathBuf,
    /// The path to the application's il2cpp metadata file (global-metadata.dat)
    metadata: PathBuf,
    /// The path to the xref data created by xref_gen
    xref_data: PathBuf,
    /// The output directory to place script and script data into
    #[clap(short, long, default_value = "./data")]
    output_dir: PathBuf,
}

fn main() -> Result<()> {
    color_eyre::install()?;
    let args = Args::parse();

    let elf_data = fs::read(&args.shared_object)?;
    let elf = Elf::parse(&elf_data)?;

    let metadata_data = fs::read(&args.metadata)?;
    let global_metadata = brocolib::global_metadata::deserialize(&metadata_data)?;
    let runtime_metadata = RuntimeMetadata::read(&elf, &global_metadata)?;
    let metadata = Metadata {
        global_metadata,
        runtime_metadata,
    };
    let cr = &metadata.runtime_metadata.code_registration;

    let xref_data = serde_json::from_str(&fs::read_to_string(&args.xref_data)?)?;
    let roots = find_roots(&metadata, &cr, &xref_data)?;

    let mut symbols = HashMap::new();
    for symbol in elf.dynamic_symbols() {
        if symbol.is_definition() {
            symbols.insert(symbol.name()?, symbol.address());
        }
    }

    let tracer = XRefTracer {
        elf: &elf,
        roots,
        symbols,
    };
    println!("tracing all symbols.");
    let output = tracer.trace_all(&xref_data)?;

    fs::write(
        args.output_dir.join("xref_apply.json"),
        serde_json::to_string(&output)?,
    )?;
    println!("trace complete.");

    // dbg!(output);
    Ok(())
}

fn find_roots<'md>(
    metadata: &'md Metadata,
    code_registration: &Il2CppCodeRegistration,
    xref_data: &XRefData,
) -> Result<Roots<'md>> {
    let global_metadata = &metadata.global_metadata;
    let mut required_roots = HashSet::new();
    for trace in &xref_data.traces {
        if trace.start.starts_with("il2cpp:") || trace.start.starts_with("invoker:") {
            let parts: Vec<&str> = trace.start.split(':').collect();
            let namespace = parts[1];
            let class = parts[2];
            let method_name = parts[3];
            required_roots.insert((namespace, class, method_name));
        }
    }

    let mut roots = HashMap::new();
    for image in global_metadata.images.as_vec() {
        let image_name = image.name(metadata);
        let type_defs = image.types(metadata);
        for type_def in type_defs {
            let namespace = type_def.namespace(metadata);
            let class = type_def.name(metadata);
            let methods = type_def.methods(metadata);
            for method in methods {
                let method_name = method.name(metadata);
                if required_roots
                    .take(&(namespace, class, method_name))
                    .is_some()
                {
                    let root = Root::get(method.token, image_name, code_registration)?;
                    roots.insert((namespace, class, method_name), root);
                }
            }
        }
    }

    Ok(roots)
}

type Roots<'a> = HashMap<(&'a str, &'a str, &'a str), Root>;

#[derive(Debug)]
struct Root {
    method_addr: u64,
    invoker_addr: Option<u64>,
}

impl Root {
    fn get(
        token: Token,
        image_name: &str,
        code_registration: &Il2CppCodeRegistration,
    ) -> Result<Self> {
        let rid = token.rid();
        let module = code_registration
            .code_gen_modules
            .iter()
            .find(|module| module.name == image_name)
            .context("could not find module for xref trace")?;

        let method_addr = module.method_pointers[rid as usize - 1];
        let invoker_idx = module.invoker_indices[rid as usize - 1];
        let invoker_addr = if invoker_idx == u32::MAX {
            None
        } else {
            Some(code_registration.invoker_pointers[invoker_idx as usize])
        };

        Ok(Self {
            method_addr,
            invoker_addr,
        })
    }
}

struct XRefTracer<'a> {
    elf: &'a Elf<'a>,
    roots: Roots<'a>,
    symbols: HashMap<&'a str, u64>,
}

impl<'a> XRefTracer<'a> {
    fn trace_all(&self, xref_data: &'a XRefData) -> Result<Output<'a>> {
        let mut symbols = Vec::new();
        for trace in &xref_data.traces {
            match self.trace_single(trace) {
                Ok(symbol) => symbols.push(symbol),
                Err(err) => eprintln!(
                    "{:?}",
                    err.wrap_err(format!(
                        "failed to trace symbol '{}' starting at '{}'",
                        trace.symbol, trace.start
                    ))
                ),
            };
        }
        Ok(Output { symbols })
    }

    fn trace_single(&self, trace: &'a SymbolTrace) -> Result<OutputSymbol<'a>> {
        let start: u64 = if trace.start.starts_with("il2cpp:") {
            let parts: Vec<&str> = trace.start.split(':').collect();
            let root = &self.roots[&(parts[1], parts[2], parts[3])];
            root.method_addr
        } else if trace.start.starts_with("invoker:") {
            let parts: Vec<&str> = trace.start.split(':').collect();
            let root = &self.roots[&(parts[1], parts[2], parts[3])];
            root.invoker_addr
                .context("root does not have invoker pointer")?
        } else {
            self.symbols[trace.start.as_str()]
        };

        let nums = trace
            .trace
            .split(|c| ('A'..='Z').contains(&c))
            .filter(|s| !s.is_empty())
            .map(|s| s.parse::<usize>());
        let ops = trace.trace.chars().filter(|&c| char::is_alphabetic(c));

        let mut addr = start;
        for (op, num) in ops.zip(nums) {
            let num = num?;
            let mut count = 0;
            loop {
                let ins = self.load_ins(addr)?;
                match ins.op() {
                    Op::BL if op == 'L' => {
                        if count == num {
                            let to = match ins.operands()[0] {
                                Operand::Label(Imm::Unsigned(to)) => to,
                                _ => bail!("bl had wrong operand"),
                            };
                            addr = to as _;
                            break;
                        }
                        count += 1;
                    }
                    Op::B if op == 'B' => {
                        if count == num {
                            let to = match ins.operands()[0] {
                                Operand::Label(Imm::Unsigned(to)) => to,
                                _ => bail!("b had wrong operand"),
                            };
                            addr = to as _;
                            break;
                        }
                        count += 1;
                    }
                    Op::ADRP if op == 'P' => {
                        if count == num {
                            let (base, reg) = match ins.operands() {
                                [Operand::Reg { reg, .. }, Operand::Label(Imm::Unsigned(imm))] => {
                                    (*imm, *reg)
                                }
                                _ => bail!("adrp had wrong operands"),
                            };
                            loop {
                                addr += 4;
                                let ins = self.load_ins(addr)?;
                                match (ins.op(), ins.operands()) {
                                    (
                                        Op::LDR,
                                        [Operand::Reg { .. }, Operand::MemOffset {
                                            reg: a,
                                            offset: Imm::Signed(imm),
                                            ..
                                        }],
                                    ) if reg == *a => {
                                        addr = ((base as i64) + imm) as _;
                                        break;
                                    }
                                    (
                                        Op::ADD,
                                        [Operand::Reg { .. }, Operand::Reg { reg: a, .. }, Operand::Imm64 {
                                            imm: Imm::Unsigned(imm),
                                            ..
                                        }],
                                    ) if reg == *a => {
                                        addr = (base + imm) as _;
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                            break;
                        }
                        count += 1;
                    }
                    _ => {}
                }
                addr += 4;
            }
        }

        Ok(OutputSymbol {
            offset: addr,
            symbol: &trace.symbol,
        })
    }

    fn load_ins(&self, addr: u64) -> Result<Instruction> {
        let addr = addr as usize;
        let data = &self.elf.data()[addr..addr + 4];
        let data = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
        bad64::decode(data, addr as u64)
            .map_err(|err| eyre!("decode error during xref walk: {}", err))
    }
}

#[derive(Deserialize, Debug)]
struct SymbolTrace {
    symbol: String,
    start: String,
    trace: String,
}

#[derive(Deserialize)]
pub struct XRefData {
    traces: Vec<SymbolTrace>,
}

#[derive(Serialize, Debug)]
struct OutputSymbol<'a> {
    symbol: &'a str,
    offset: u64,
}

#[derive(Serialize, Debug)]
struct Output<'a> {
    symbols: Vec<OutputSymbol<'a>>,
}
