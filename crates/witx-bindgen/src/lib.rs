use heck::*;
use std::io::{Read, Write};
use std::mem;
use std::path::Path;
use std::process::{Command, Stdio};
use witx::*;

pub fn generate<P: AsRef<Path>>(witx_paths: &[P]) -> String {
    let doc = witx::load(witx_paths).unwrap();

    let mut raw = String::new();
    raw.push_str(
        "\
// This file is automatically generated, DO NOT EDIT
//
// To regenerate this file run the `crates/witx-bindgen` command
package kotlinx.wasi

import kotlin.wasm.unsafe.*
import kotlin.wasm.WasmImport

",
    );
    for ty in doc.typenames() {
        ty.render(&mut raw);
        raw.push_str("\n");
    }
    for m in doc.modules() {
        m.render(&mut raw);
        raw.push_str("\n");
    }
    for c in doc.constants() {
        rustdoc(&c.docs, &mut raw);
        raw.push_str(&format!(
            "const val {}_{}: {} = {}\n",
            c.ty.as_str().to_shouty_snake_case(),
            c.name.as_str().to_shouty_snake_case(),
            c.ty.as_str().to_camel_case(),
            c.value
        ));
    }

    return raw;
}

trait Render {
    fn render(&self, src: &mut String);
}

trait IsSafe {
    fn is_safe(&self) -> bool;
}

impl IsSafe for Type {
    fn is_safe(&self) -> bool {
        match self {
            Type::Record(record) =>
                record.members.iter().all(|it| it.tref.is_safe()),

            Type::Variant(variant) =>
                variant.cases.iter().all(|it|
                    it.tref.is_none() || it.tref.as_ref().unwrap().is_safe()
                ),

            Type::Handle(_) => true,
            Type::List(l) => l.type_().is_safe(),
            Type::Pointer(_) => false,
            Type::ConstPointer(_) => false,
            Type::Builtin(_) => true,
        }
    }
}

impl IsSafe for InterfaceFuncParam {
    fn is_safe(&self) -> bool {
        self.tref.is_safe()
    }
}

impl IsSafe for InterfaceFunc {
    fn is_safe(&self) -> bool {
        self.params.iter().all(|it| it.is_safe()) &&
            self.results.iter().all(|it| it.is_safe())
    }
}

impl IsSafe for TypeRef {
    fn is_safe(&self) -> bool {
        self.type_().is_safe()
    }
}

impl IsSafe for RecordDatatype {
    fn is_safe(&self) -> bool {
        self.members.iter().all(|it| it.tref.is_safe())
    }
}

impl Render for NamedType {
    fn render(&self, src: &mut String) {
        let name = self.name.as_str();
        match &self.tref {
            TypeRef::Value(ty) => match &**ty {
                Type::Record(s) => render_record(src, name, self, s),
                Type::Handle(h) => render_handle(src, name, h),
                Type::Variant(h) => render_variant(src, name, h),
                Type::List { .. }
                | Type::Pointer { .. }
                | Type::ConstPointer { .. }
                | Type::Builtin { .. } => render_alias(src, name, &self.tref),
            },
            TypeRef::Name(_nt) => render_alias(src, name, &self.tref),
        }
    }
}

fn render_record(src: &mut String, name: &str, nt: &NamedType, s: &RecordDatatype) {
    if let Some(repr) = s.bitflags_repr() {
        src.push_str(&format!("typealias {} = ", name.to_camel_case()));
        repr.render(src);
        src.push('\n');

        src.push_str("object ");
        src.push_str(name.to_shouty_snake_case().as_str());
        src.push_str(" {\n");
        for (i, member) in s.members.iter().enumerate() {
            rustdoc(&member.docs, src);
            src.push_str(&format!(
                "const val {}: {} = 1 shl {}\n",
                member.name.as_str().to_shouty_snake_case(),
                name.to_camel_case(),
                i,
            ));
        }
        src.push_str("}\n");
        return;
    }
    if !s.is_safe() {
        src.push_str("internal ");
    }
    src.push_str("data class ");

    let full_name = if !s.is_safe() {
        "__unsafe__"
    } else {
        ""
    }.to_string() + name.to_camel_case().as_str();

    src.push_str(&format!("{}(\n", full_name));
    for member in s.members.iter() {
        rustdoc(&member.docs, src);
        src.push_str("var ");
        member.name.render(src);
        src.push_str(": ");
        member.tref.render(src);
        src.push_str(",\n");
    }
    src.push_str(")");

    src.push_str(&*format!("internal fun __load_{}(ptr: Int): {} {{\n", full_name, full_name));
    src.push_str("    return ");
    load_type(nt, "ptr", src);
    src.push_str("\n}\n");

    src.push_str(&*format!("internal fun __store_{}(x: {}, ptr: Int) {{\n", full_name, full_name));
    store_type(nt, "x", "ptr", src);
    src.push_str("\n}\n");
}

fn is_variant_enum_like(v: &Variant) -> bool {
    return v.cases.iter().all(|c| c.tref.is_none());
}

fn render_variant(src: &mut String, name: &str, v: &Variant) {
    if is_variant_enum_like(v) {
        return render_enum_like_variant(src, name, v);
    }
    src.push_str(&format!("sealed class {} {{\n", name.to_camel_case()));
    for case in v.cases.iter() {
        if let Some(ref tref) = case.tref {
            rustdoc(&case.docs, src);
            src.push_str("data class ");
            case.name.render(src);
            src.push_str("(var value: ");
            tref.render(src);
            src.push_str(&format!(") : {}()\n", name.to_camel_case()));
        }
    }
    src.push_str("}\n");
}

fn render_enum_like_variant(src: &mut String, name: &str, s: &Variant) {
    src.push_str(&format!("enum class {} {{\n", name.to_camel_case()));
    for (i, variant) in s.cases.iter().enumerate() {
        rustdoc(&variant.docs, src);
        if variant.name.as_str().as_bytes()[0].is_ascii_digit() {
            src.push_str("_");
        }
        src.push_str(variant.name.as_str().to_shouty_snake_case().as_str());
        src.push_str(",\n");
    }
    src.push_str("}\n");
}

impl Render for IntRepr {
    fn render(&self, src: &mut String) {
        match self {
            IntRepr::U8 => src.push_str("Byte"),
            IntRepr::U16 => src.push_str("Short"),
            IntRepr::U32 => src.push_str("Int"),
            IntRepr::U64 => src.push_str("Long"),
        }
    }
}

fn render_alias(src: &mut String, name: &str, dest: &TypeRef) {
    if !dest.is_safe() {
        src.push_str("internal ")
    }
    src.push_str("typealias ");
    if !dest.is_safe() {
        src.push_str("__unsafe__")
    }
    src.push_str(name.to_camel_case().as_str());
    src.push_str(" = ");
    dest.render(src);
}

impl Render for TypeRef {
    fn render(&self, src: &mut String) {
        match self {
            TypeRef::Name(t) => {
                if !self.is_safe() {
                    src.push_str("__unsafe__")
                }
                src.push_str(&t.name.as_str().to_camel_case());
            }
            TypeRef::Value(v) => match &**v {
                Type::Builtin(t) => t.render(src),
                Type::List(t) => match &**t.type_() {
                    Type::Builtin(BuiltinType::Char) => src.push_str("String"),
                    _ => {
                        src.push_str("List<");
                        t.render(src);
                        src.push_str(">");
                    }
                },
                Type::Pointer(t) => {
                    src.push_str("Pointer/*<");
                    t.render(src);
                    src.push_str(">*/");
                }
                Type::ConstPointer(t) => {
                    src.push_str("Pointer/*<");
                    t.render(src);
                    src.push_str(">*/");
                }
                Type::Variant(v) if v.is_bool() => src.push_str("Boolean"),
                Type::Variant(v) => match v.as_expected() {
                    Some((ok, err)) => {
                        match ok {
                            Some(ty) => ty.render(src),
                            None => src.push_str("Unit"),
                        }
                    }
                    None => {
                        panic!("unsupported anonymous variant")
                    }
                },
                Type::Record(r) if r.is_tuple() => {
                    if r.members.len() != 2 { unimplemented!() }

                    src.push_str(&format!("Pair<"));
                    for member in r.members.iter() {
                        member.tref.render(src);
                        src.push_str(",");
                    }
                    src.push_str(">");
                }
                t => panic!("reference to anonymous {} not possible!", t.kind()),
            },
        }
    }
}

impl Render for BuiltinType {
    fn render(&self, src: &mut String) {
        match self {
            // A C `char` in Rust we just interpret always as `u8`. It's
            // technically possible to use `std::os::raw::c_char` but that's
            // overkill for the purposes that we'll be using this type for.
            BuiltinType::U8 { lang_c_char: _ } => src.push_str("Byte"),
            BuiltinType::U16 => src.push_str("Short"),
            BuiltinType::U32 {
                lang_ptr_size: false,
            } => src.push_str("Int"),
            BuiltinType::U32 {
                lang_ptr_size: true,
            } => src.push_str("Int"),
            BuiltinType::U64 => src.push_str("Long"),
            BuiltinType::S8 => src.push_str("Byte"),
            BuiltinType::S16 => src.push_str("Short"),
            BuiltinType::S32 => src.push_str("Int"),
            BuiltinType::S64 => src.push_str("Long"),
            BuiltinType::F32 => src.push_str("Float"),
            BuiltinType::F64 => src.push_str("Double"),
            BuiltinType::Char => panic!("Chars are unsupported"),
        }
    }
}

impl Render for Module {
    fn render(&self, src: &mut String) {
        // wrapper functions
        for f in self.funcs() {
            render_highlevel(&f, &self.name, src);
            src.push_str("\n\n");
        }

        // raw module
        for f in self.funcs() {
            f.render(src);
            src.push_str("\n");
        }
    }
}

fn render_highlevel(func: &InterfaceFunc, module: &Id, src: &mut String) {
    rustdoc(&func.docs, src);
    // TODO[Kotlin] Docs
    rustdoc_params(&func.params, "Parameters", src);
    rustdoc_params(&func.results, "Return", src);

    let is_safe = func.is_safe();

    // Render the function and its arguments, and note that the arguments here
    // are the exact type name arguments as opposed to the pointer/length pair
    // ones. These functions are unsafe because they work with integer file
    // descriptors, which are effectively forgeable and danglable raw pointers
    // into the file descriptor address space.

    if !is_safe {
        src.push_str("internal ")
    }
    src.push_str("fun ");

    // TODO workout how to handle wasi-ephemeral which introduces multiple
    // WASI modules into the picture. For now, feature-gate it, and if we're
    // compiling ephmeral bindings, prefix wrapper syscall with module name.
    let kotlin_name = func.name.as_str();
    if !is_safe {
        src.push_str("__unsafe__")
    }
    if cfg!(feature = "multi-module") {
        src.push_str(&[module.as_str().to_snake_case().as_str(), &kotlin_name].join("_"));
    } else {
        src.push_str(to_rust_ident(&kotlin_name));
    }

    src.push_str("(");
    if !is_safe {
        src.push_str("allocator: MemoryAllocator,")
    }
    for param in func.params.iter() {
        param.name.render(src);
        src.push_str(": ");
        param.tref.render(src);
        src.push_str(",");
    }
    src.push_str(")");

    match func.results.len() {
        0 => {}
        1 => {
            src.push_str(": ");
            func.results[0].tref.render(src);
        }
        _ => { unimplemented!() }
    }
    src.push_str(" {\n");

    if is_safe {
        src.push_str("withScopedMemoryAllocator { allocator ->")
    }
    func.call_wasm(
        module,
        &mut Rust {
            src,
            params: &func.params,
            block_storage: Vec::new(),
            blocks: Vec::new(),
        },
    );

    if is_safe {
        src.push_str("}") // END withScopedMemoryAllocator
    }


    src.push_str("}");
}

struct Rust<'a> {
    src: &'a mut String,
    params: &'a [InterfaceFuncParam],
    block_storage: Vec<String>,
    blocks: Vec<String>,
}

impl Bindgen for Rust<'_> {
    type Operand = String;

    fn push_block(&mut self) {
        let prev = mem::replace(self.src, String::new());
        self.block_storage.push(prev);
    }

    fn finish_block(&mut self, operand: Option<String>) {
        let to_restore = self.block_storage.pop().unwrap();
        let src = mem::replace(self.src, to_restore);
        match operand {
            None => {
                assert!(src.is_empty());
                self.blocks.push("Unit".to_string());
            }
            Some(s) => {
                if src.is_empty() {
                    self.blocks.push(s);
                } else {
                    self.blocks.push(format!("{{ {};\n {} }}", src, s));
                }
            }
        }
    }

    fn allocate_space(&mut self, n: usize, ty: &witx::NamedType) {
        self.src.push_str(&format!("val rp{} = allocator.allocate({})\n", n, ty.mem_size()));
    }

    fn emit(
        &mut self,
        inst: &Instruction<'_>,
        operands: &mut Vec<String>,
        results: &mut Vec<String>,
    ) {
        let mut top_as = |cvt: &str| {
            let mut s = operands.pop().unwrap();
            s.push_str(".to");
            s.push_str(cvt);
            s.push_str("()");
            results.push(s);
        };

        match inst {
            Instruction::GetArg { nth } => {
                let mut s = String::new();
                self.params[*nth].name.render(&mut s);
                results.push(s);
            }
            Instruction::AddrOf => {
                panic!("Instruction::AddrOf unsupported");
            }
            Instruction::I64FromBitflags { .. } | Instruction::I64FromU64 => top_as("Long"),
            Instruction::I32FromPointer
            | Instruction::I32FromConstPointer
            | Instruction::I32FromHandle { .. }
            | Instruction::I32FromUsize
            | Instruction::I32FromChar
            | Instruction::I32FromU8
            | Instruction::I32FromS8
            | Instruction::I32FromChar8
            | Instruction::I32FromU16
            | Instruction::I32FromS16
            | Instruction::I32FromU32
            | Instruction::I32FromBitflags { .. } => top_as("Int"),

            Instruction::EnumLower { .. } => {
                results.push(format!("{}.ordinal", operands[0]));
            }

            Instruction::F32FromIf32
            | Instruction::F64FromIf64
            | Instruction::If32FromF32
            | Instruction::If64FromF64
            | Instruction::I64FromS64
            | Instruction::I32FromS32 => {
                results.push(operands.pop().unwrap());
            }
            Instruction::ListPointerLength => {
                let list = operands.pop().unwrap();
                results.push(format!("allocator.writeToLinearMemory({})", list));
                results.push(format!("{}.size", list));
            }
            Instruction::S8FromI32 => top_as("Byte"),
            Instruction::Char8FromI32 | Instruction::U8FromI32 => top_as("Byte"),
            Instruction::S16FromI32 => top_as("Short"),
            Instruction::U16FromI32 => top_as("Short"),
            Instruction::S32FromI32 => {}
            Instruction::U32FromI32 => {},
            Instruction::S64FromI64 => {}
            Instruction::U64FromI64 => {},
            Instruction::UsizeFromI32 => {},
            Instruction::HandleFromI32 { .. } => {},
            Instruction::PointerFromI32 { .. } => {},
            Instruction::ConstPointerFromI32 { .. } => {},
            Instruction::BitflagsFromI32 { .. } => unimplemented!(),
            Instruction::BitflagsFromI64 { .. } => unimplemented!(),

            Instruction::ReturnPointerGet { n } => {
                results.push(format!("rp{}", n));
            }

            Instruction::Load { ty } => {
                // let mut s = format!("core::ptr::read({} as *const ", &operands[0]);
                let mut s = format!("");
                match &ty.tref {
                    TypeRef::Name(_) => unimplemented!(),
                    TypeRef::Value(rvt) => {
                        let vt: &Type = rvt.as_ref();
                        load_type(&ty, &operands[0], &mut s);
                    },
                }
                results.push(s);
            }

            Instruction::ReuseReturn => {
                results.push("ret".to_string());
            }

            Instruction::TupleLift { .. } => {
                if operands.len() != 2 {
                    unimplemented!();
                }

                let value = format!("Pair({})", operands.join(", "));
                results.push(value);
            }

            Instruction::ResultLift => {
                let err = self.blocks.pop().unwrap();
                let ok = self.blocks.pop().unwrap();

                let mut result = format!("if ({} == 0) {{\n", operands[0]);
                result.push_str(&ok);
                result.push_str("} else {");
                result.push_str("throw WasiError(");
                result.push_str(&err);
                result.push_str(")\n");
                result.push_str("}\n");
                results.push(result);
            }

            Instruction::EnumLift { ty } => {
                let mut result = ty.name.as_str().to_camel_case();
                result.push_str(".values()[");
                result.push_str(&operands[0]);
                result.push_str("]");
                results.push(result);
            }

            Instruction::CharFromI32 => unimplemented!(),

            Instruction::CallWasm {
                module,
                name,
                params: _,
                results: func_results,
            } => {
                assert!(func_results.len() < 2);
                if func_results.len() > 0 {
                    self.src.push_str("val ret = ");
                    results.push("ret".to_string());
                }
                self.src.push_str("_raw_wasm__");
                self.src.push_str(name);
                self.src.push_str("(");
                self.src.push_str(&operands.join(", "));
                self.src.push_str(");\n");
            }

            Instruction::Return { amt: 0 } => {}
            Instruction::Return { amt: 1 } => {
                self.src.push_str("return ");
                self.src.push_str(&operands[0]);
            }
            Instruction::Return { .. } => {
                unimplemented!();
                self.src.push_str("(");
                self.src.push_str(&operands.join(", "));
                self.src.push_str(")");
            }

            Instruction::Store { .. }
            | Instruction::ListFromPointerLength { .. }
            | Instruction::CallInterface { .. }
            | Instruction::ResultLower { .. }
            | Instruction::TupleLower { .. }
            | Instruction::VariantPayload => unimplemented!(),
        }
    }
}

fn load_int_repr(repr: IntRepr, ptr: &str, s: &mut String) {
    let fun = match repr {
        IntRepr::U8 => "loadByte",
        IntRepr::U16 => "loadShort",
        IntRepr::U32 => "loadInt",
        IntRepr::U64 => "loadLong"
    };

    s.push_str(format!("{}({})", fun, ptr).as_str());
}

fn store_int_repr(repr: IntRepr, reference: &str, ptr: &str, s: &mut String) {
    let fun = match repr {
        IntRepr::U8 => "storeByte",
        IntRepr::U16 => "storeShort",
        IntRepr::U32 => "storeInt",
        IntRepr::U64 => "storeLong"
    };

    s.push_str(format!("{}({}, {})", fun, ptr, reference).as_str());
}

fn to_kotlin_type(repr: IntRepr) -> &'static str {
    match repr {
        IntRepr::U8 => "Byte",
        IntRepr::U16 => "Short",
        IntRepr::U32 => "Int",
        IntRepr::U64 => "Long"
    }
}

fn load_type_ref(tr: &TypeRef, ptr: &str, s: &mut String) {
    match tr {
        TypeRef::Name(nt) => {
            load_type(nt, ptr, s);
        }
        TypeRef::Value(vt) => {
            load_value_type(vt, ptr, s);
        }
    }
}

fn store_type_ref(tr: &TypeRef, reference: &str, ptr: &str, s: &mut String) {
    match tr {
        TypeRef::Name(nt) => {
            store_type(nt, reference, ptr, s);
        }
        TypeRef::Value(vt) => {
            store_value_type(vt, reference, ptr, s);
        }
    }
}

fn load_value_type(vt: &Type, ptr: &str, s: &mut String) {
    match vt {
        Type::Record(_) => { unimplemented!(); }
        Type::Variant(_) => { unimplemented!(); }
        Type::Handle(_) => {
            load_handle(ptr, s);
        }
        Type::List(l) => {
            unimplemented!();
        }
        Type::Pointer(_) => {
            load_int_repr(IntRepr::U32, ptr, s);
        }
        Type::ConstPointer(cp) => {
            load_int_repr(IntRepr::U32, ptr, s);
        }
        Type::Builtin(b) => {
            load_built_in(b, ptr, s);

        }
    }
}

fn store_value_type(vt: &Type, reference: &str, ptr: &str, s: &mut String) {
    match vt {
        Type::Record(_) => { unimplemented!(); }
        Type::Variant(_) => { unimplemented!(); }
        Type::List(l) => {
            unimplemented!();
        }
        Type::Handle(_) | Type::Pointer(_) | Type::ConstPointer(_) => {
            store_int_repr(IntRepr::U32, reference, ptr, s);
        }
        Type::Builtin(b) => {
            store_built_in(b, reference, ptr, s);
        }
    }
}

trait KotlinName {
    fn kotlin_name(&self) -> &String;
}

fn store_type(nt: &NamedType, reference: &str, ptr: &str, s: &mut String) {
    match nt.tref.type_().as_ref() {
        Type::Record(record) => {
            if record.is_tuple() {
                unimplemented!();
            }

            if let Some(repr) = record.bitflags_repr() {
                store_int_repr(repr, reference, ptr, s);
                return;
            } else {
                let member_layouts = record.member_layout();
                for (i, member) in record.members.iter().enumerate() {
                    let layout = &member_layouts[i];
                    let member_ref = format!("{}.{}", reference, to_rust_ident(member.name.as_str()));
                    let member_ptr = format!("{} + {}", ptr, layout.offset);
                    store_type_ref(&member.tref, member_ref.as_str(), member_ptr.as_str(), s);
                    s.push_str("\n");
                }
            }
        }
        Type::Variant(variant) => {
            if is_variant_enum_like(variant) {
                let var_ref = format!("{}.ordinal.to{}()", reference, to_kotlin_type(variant.tag_repr));
                store_int_repr(variant.tag_repr,var_ref.as_str(), ptr, s);
            } else {
                s.push_str("TODO()")
            }
        }
        Type::Handle(_) => {
            store_int_repr(IntRepr::U32, reference, ptr, s);
        }
        Type::List(l) => {
            unimplemented!();
        }
        Type::Pointer(_) => {
            unimplemented!();
        }
        Type::ConstPointer(_) => {
            unimplemented!();
        }
        Type::Builtin(builtin) => {
            store_built_in(builtin, reference, ptr, s);
        }
    }
}

fn load_type(nt: &NamedType, ptr: &str, s: &mut String) {
    let t = nt.type_().as_ref();
    let is_safe = nt.tref.is_safe();
    let safe_prefix = if is_safe { "" } else { "__unsafe__" };
    match t {
        Type::Record(record) => {
            if record.is_tuple() {
                unimplemented!();
            }

            if let Some(repr) = record.bitflags_repr() {
                load_int_repr(repr, ptr, s);
                return;
            } else {
                s.push_str(safe_prefix);
                s.push_str(nt.name.as_str().to_camel_case().as_str());
                s.push_str("(");
                let member_layouts = record.member_layout();
                for (i, member) in record.members.iter().enumerate() {
                    let layout = &member_layouts[i];
                    let member_ptr = format!("{} + {}", ptr, layout.offset);
                    load_type_ref(&member.tref, member_ptr.as_str(), s);
                    s.push_str(",");
                }
                s.push_str(")");
            }
        }
        Type::Variant(variant) => {
            if is_variant_enum_like(variant) {
                s.push_str(nt.name.as_str().to_camel_case().as_str());
                s.push_str(".values()[");
                load_int_repr(variant.tag_repr, ptr, s);
                s.push_str(".toInt()]");
            } else {
                s.push_str("when (");
                load_int_repr(variant.tag_repr, ptr, s);
                s.push_str(".toInt()) { ");
                for (i, case) in variant.cases.iter().enumerate() {
                    s.push_str(i.to_string().as_str());
                    s.push_str(" -> { ");
                    let offset = variant.payload_offset();
                    let tref = &case.tref;
                    s.push_str(format!(
                        "{}.{}(",
                        nt.name.as_str().to_camel_case().as_str(),
                        case.name.as_str(),
                    ).as_str());
                    load_type_ref(tref.as_ref().unwrap(), format!("{} + {}", ptr, offset).as_str(), s);
                    s.push_str(")");
                    s.push_str("} ");
                }
                s.push_str("else -> error(\"Invalid variant\")");
                s.push_str("}");
            }
        }
        Type::Handle(_) => {
            load_handle(ptr, s);
        }
        Type::List(_) => {
            unimplemented!();
        }
        Type::Pointer(_) => {
            unimplemented!();
        }
        Type::ConstPointer(_) => {
            unimplemented!();
        }
        Type::Builtin(builtin) => {
            load_built_in(builtin, ptr, s);
        }
    }
}

fn load_built_in(builtin: &BuiltinType, ptr: &str, src: &mut String) {
    match builtin {
        BuiltinType::U32 { .. } => {
            load_int_repr(IntRepr::U32, ptr, src);
        }
        BuiltinType::U64 => {
            load_int_repr(IntRepr::U64, ptr, src);
        }
        _ => { unimplemented!(); }
    }
}

fn store_built_in(builtin: &BuiltinType, reference: &str, ptr: &str, src: &mut String) {
    match builtin {
        BuiltinType::U32 { .. } => {
            store_int_repr(IntRepr::U32, reference, ptr, src);
        }
        BuiltinType::U64 => {
            store_int_repr(IntRepr::U64, reference, ptr, src);
        }
        _ => { unimplemented!(); }
    }
}

fn load_handle(ptr: &str, src: &mut String) {
    load_int_repr(IntRepr::U32, ptr, src);
}

impl Render for InterfaceFunc {
    fn render(&self, src: &mut String) {
        rustdoc(&self.docs, src);
        if self.name.as_str() != self.name.as_str().to_snake_case() {
            panic!("Unsupported!");
        }
        src.push_str(format!(
            "@WasmImport(\"wasi_snapshot_preview1\", \"{}\")\n",
            self.name.as_str()
        ).as_str());
        src.push_str("private external fun ");
        src.push_str("_raw_wasm__");
        src.push_str(self.name.as_str());

        let (params, results) = self.wasm_signature();
        assert!(results.len() <= 1);
        src.push_str("(");
        for (i, param) in params.iter().enumerate() {
            src.push_str(&format!("arg{}: ", i));
            param.render(src);
            src.push_str(",");
        }
        src.push_str(")");

        if self.noreturn {
            src.push_str(": Unit");
        } else if let Some(result) = results.get(0) {
            src.push_str(": ");
            result.render(src);
        }
    }
}

fn to_rust_ident(name: &str) -> &str {
    match name {
        "in" => "in_",
        s => s
    }
}

impl Render for Id {
    fn render(&self, src: &mut String) {
        src.push_str(to_rust_ident(self.as_str()))
    }
}

impl Render for WasmType {
    fn render(&self, src: &mut String) {
        match self {
            WasmType::I32 => src.push_str("Int"),
            WasmType::I64 => src.push_str("Long"),
            WasmType::F32 => src.push_str("Float"),
            WasmType::F64 => src.push_str("Double"),
        }
    }
}

fn render_handle(src: &mut String, name: &str, _h: &HandleDatatype) {
    src.push_str(&format!("typealias {} = Int", name.to_camel_case()));
}

fn rustdoc(docs: &str, dst: &mut String) {
    if docs.trim().is_empty() {
        return;
    }

    dst.push_str("/**\n");
    for line in docs.lines() {
        dst.push_str(" * ");
        dst.push_str(line);
        dst.push_str("\n");
    }
    dst.push_str(" */\n");
}

fn rustdoc_params(docs: &[InterfaceFuncParam], header: &str, dst: &mut String) {
    let docs = docs
        .iter()
        .filter(|param| param.docs.trim().len() > 0)
        .collect::<Vec<_>>();
    if docs.len() == 0 {
        return;
    }

    dst.push_str("///\n");
    dst.push_str("/// ## ");
    dst.push_str(header);
    dst.push_str("\n");
    dst.push_str("///\n");

    for param in docs {
        for (i, line) in param.docs.lines().enumerate() {
            dst.push_str("/// ");
            // Currently wasi only has at most one return value, so there's no
            // need to indent it or name it.
            if header != "Return" {
                if i == 0 {
                    dst.push_str("* `");
                    param.name.render(dst);
                    dst.push_str("` - ");
                } else {
                    dst.push_str("  ");
                }
            }
            dst.push_str(line);
            dst.push_str("\n");
        }
    }
}

fn record_contains_union(s: &RecordDatatype) -> bool {
    s.members
        .iter()
        .any(|member| type_contains_union(&member.tref.type_()))
}

fn type_contains_union(ty: &Type) -> bool {
    match ty {
        Type::Variant(c) => c.cases.iter().any(|c| c.tref.is_some()),
        Type::List(tref) => type_contains_union(&tref.type_()),
        Type::Record(st) => record_contains_union(st),
        _ => false,
    }
}
