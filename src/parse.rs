use elf::abi::*;
use elf::endian::EndianParse;
use elf::relocation::RelaIterator;
use elf::ElfBytes;

use crate::repr::*;

pub fn parse_elf<E: EndianParse>(elf_data: &[u8]) -> Image {
    let elf_file = ElfBytes::<E>::minimal_parse(elf_data).expect("Unparseable ELF");
    let elf_common = elf_file.find_common_data().expect("No ELF common data");
    let segments = elf_file.segments()
        .expect("No segments")
        .iter()
        .filter_map(|elf_segment| {
            if elf_segment.p_type == PT_LOAD {
                let addr = elf_segment.p_vaddr;
                let size = elf_segment.p_memsz;
                let data = elf_file.segment_data(&elf_segment)
                    .expect("No data for PT_LOAD")
                    .to_owned();
                let mode = if elf_segment.p_flags == PF_R {
                    LoadMode::ReadOnly
                } else if elf_segment.p_flags == PF_R | PF_W {
                    LoadMode::ReadWrite
                } else if elf_segment.p_flags == PF_R | PF_X {
                    LoadMode::ReadExecute
                } else {
                    panic!("Unknown segment flags: {}",
                        elf::to_str::p_flags_to_string(elf_segment.p_flags))
                };
                Some(LoadSegment { addr, size, data, mode })
            } else {
                None
            }

        })
        .collect::<Vec<_>>();
    let elf_dynsyms = elf_common.dynsyms.as_ref().expect("No dynamic symbol table");
    let elf_dynsyms_strs = elf_common.dynsyms_strs.as_ref().expect("No dynamic symbol string table");
    let symbols = elf_dynsyms
        .clone()
        .into_iter()
        .filter_map(|elf_symbol| {
            let elf_symtype = elf_symbol.st_symtype();
            if elf_symtype == STT_FUNC || elf_symtype == STT_OBJECT {
                let name = elf_dynsyms_strs
                    .get(elf_symbol.st_name as usize)
                    .expect("Invalid symbol name")
                    .to_owned();
                let kind = if elf_symtype == STT_FUNC {
                    SymbolKind::Code
                } else {
                    SymbolKind::Data
                };
                let value = elf_symbol.st_value;
                let elf_symvis = elf_symbol.st_vis();
                let scope = if elf_symvis == STB_LOCAL {
                    SymbolScope::Local
                } else if elf_symvis == STB_GLOBAL {
                    SymbolScope::Global
                } else if elf_symvis == STB_WEAK {
                    SymbolScope::Weak
                } else {
                    panic!("Unhandled symbol visibility: {}",
                        elf::to_str::st_vis_to_str(elf_symbol.st_vis())
                        .unwrap_or("<unknown>"))
                };
                Some(Symbol { name, kind, value, scope })
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    let elf_dynamic = elf_common.dynamic.map(|elf_dynamic| {
        elf_dynamic.into_iter().collect::<Vec<_>>()
    }).unwrap_or(Vec::new());
    let needed = elf_dynamic.iter().filter_map(|elf_dyn| {
        if elf_dyn.d_tag == DT_NEEDED {
            Some(elf_dynsyms_strs
                .get(elf_dyn.clone().d_val() as usize)
                .expect("Invalid DT_NEEDED name")
                .to_owned())
        } else {
            None
        }
    }).collect::<Vec<_>>();
    let parse_elf_rela = |elf_rela_data| {
        RelaIterator::new(elf_file.ehdr.endianness, elf_file.ehdr.class, elf_rela_data)
            .map(|elf_rela| {
                let offset = elf_rela.r_offset;
                let target = if elf_file.ehdr.e_machine == EM_X86_64 {
                    let symbol = if elf_rela.r_sym == 0 { None } else {
                        let elf_symbol = elf_dynsyms
                            .get(elf_rela.r_sym as usize)
                            .expect("Invalid symbol reference in relocation");
                        let elf_symbol_name = elf_dynsyms_strs
                            .get(elf_symbol.st_name as usize)
                            .expect("Invalid symbol name in relocation");
                        Some(elf_symbol_name.to_owned())
                    };
                    if elf_rela.r_type == R_X86_64_GLOB_DAT {
                        RelocationTarget::Symbol {
                            symbol: symbol.expect("R_X86_64_GLOB_DAT requires a symbol"),
                            addend: elf_rela.r_addend
                        }
                    } else if elf_rela.r_type == R_X86_64_JUMP_SLOT {
                        RelocationTarget::Symbol {
                            symbol: symbol.expect("R_X86_64_JUMP_SLOT requires a symbol"),
                            addend: elf_rela.r_addend
                        }
                    } else if elf_rela.r_type == R_X86_64_RELATIVE {
                        assert!(elf_rela.r_sym == 0, "R_X86_64_RELATIVE accepts no symbol");
                        RelocationTarget::Base { addend: elf_rela.r_addend }
                    } else {
                        panic!("Unhandled relocation type: {}", elf_rela.r_type)
                    }
                } else {
                    panic!("Unhandled machine for RELA relocations: {}",
                        elf::to_str::e_machine_to_str(elf_file.ehdr.e_machine)
                        .unwrap_or("<unknown>"))
                };
                Relocation { offset, target }
            })
            .collect::<Vec<_>>()
    };
    let elf_dynamic_rela = elf_dynamic.iter().find_map(|elf_dyn| {
        if elf_dyn.d_tag == DT_RELA { Some(elf_dyn.clone().d_val() as usize) } else { None }
    });
    let elf_dynamic_relasz = elf_dynamic.iter().find_map(|elf_dyn| {
        if elf_dyn.d_tag == DT_RELASZ { Some(elf_dyn.clone().d_val() as usize) } else { None }
    });
    let elf_dynamic_pltrel = elf_dynamic.iter().find_map(|elf_dyn| {
        if elf_dyn.d_tag == DT_PLTREL { Some(elf_dyn.clone().d_val() as i64) } else { None }
    });
    let elf_dynamic_jmprel = elf_dynamic.iter().find_map(|elf_dyn| {
        if elf_dyn.d_tag == DT_JMPREL { Some(elf_dyn.clone().d_val() as usize) } else { None }
    });
    let elf_dynamic_pltrelsz = elf_dynamic.iter().find_map(|elf_dyn| {
        if elf_dyn.d_tag == DT_PLTRELSZ { Some(elf_dyn.clone().d_val() as usize) } else { None }
    });
    let mut data_relocations = match (elf_dynamic_rela, elf_dynamic_relasz) {
        (Some(elf_dynamic_rela), Some(elf_dynamic_relasz)) =>
            parse_elf_rela(&elf_data[elf_dynamic_rela..elf_dynamic_rela + elf_dynamic_relasz]),
        (None, None) => Vec::new(),
        _ => panic!("Expected dynamic table to have both or neither of PT_RELA and PT_RELASZ")
    };
    let mut code_relocations = match (elf_dynamic_pltrel, elf_dynamic_jmprel, elf_dynamic_pltrelsz) {
        (Some(elf_dynamic_pltrel), Some(elf_dynamic_jmprel), Some(elf_dynamic_pltrelsz))
                if elf_dynamic_pltrel == DT_RELA => {
            let elf_jmprel_data = &elf_data[elf_dynamic_jmprel..elf_dynamic_jmprel + elf_dynamic_pltrelsz];
            if elf_dynamic_pltrel == DT_RELA {
                parse_elf_rela(elf_jmprel_data)
            // } else if elf_dynamic_pltrel == DT_REL {
            //     parse_elf_rel(elf_pltrel_data)
            } else {
                panic!("Unhangled PLT relocation type: {}",
                    elf::to_str::d_tag_to_str(elf_dynamic_pltrel)
                    .unwrap_or("<unknown>"));
            }
        }
        (None, None, None) => Vec::new(),
        _ => panic!("Expected dynamic table to have all or none of PT_PLTREL, PT_JMPREL and PT_PLTRELSZ")
    };
    let mut relocations = Vec::new();
    relocations.append(&mut data_relocations);
    relocations.append(&mut code_relocations);
    Image {
        segments,
        symbols,
        needed,
        relocations,
    }
}
