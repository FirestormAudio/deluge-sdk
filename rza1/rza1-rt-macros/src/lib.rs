//! Attribute macros for `rza1-rt`. Use them through `rza1_rt::{entry, irq, fault}`.

use proc_macro::TokenStream;
use proc_macro2::{Span, TokenStream as TokenStream2};
use quote::{format_ident, quote};
use syn::{FnArg, ItemFn, ReturnType, Safety, Signature, Type};

fn error(span: Span, msg: &str) -> TokenStream2 {
    syn::Error::new(span, msg).to_compile_error()
}

/// Parses `item` as a free function under an attribute that takes no arguments, or returns the
/// error to emit in its place.
fn parse(attr: TokenStream2, item: TokenStream2, name: &str) -> Result<ItemFn, TokenStream2> {
    if !attr.is_empty() {
        return Err(error(Span::call_site(), &format!("`#[{name}]` takes no arguments")));
    }
    syn::parse2(item).map_err(|e| e.to_compile_error())
}

/// No `const`, `async`, `extern`, generics or variadics: the exported symbol has a fixed C
/// signature, so nothing can vary.
fn plain(sig: &Signature) -> bool {
    sig.constness.is_none()
        && sig.asyncness.is_none()
        && sig.abi.is_none()
        && sig.generics.params.is_empty()
        && sig.generics.where_clause.is_none()
        && sig.variadic.is_none()
}

fn returns_never(output: &ReturnType) -> bool {
    matches!(output, ReturnType::Type(_, ty) if matches!(**ty, Type::Never(_)))
}

fn one_typed_input(sig: &Signature) -> bool {
    sig.inputs.len() == 1 && matches!(sig.inputs.first(), Some(FnArg::Typed(_)))
}

fn only_input_is_u32(sig: &Signature) -> bool {
    match sig.inputs.first() {
        Some(FnArg::Typed(arg)) if sig.inputs.len() == 1 => {
            matches!(&*arg.ty, Type::Path(p) if p.qself.is_none() && p.path.is_ident("u32"))
        }
        _ => false,
    }
}

fn expand_entry(attr: TokenStream2, item: TokenStream2) -> TokenStream2 {
    let mut f = match parse(attr, item, "entry") {
        Ok(f) => f,
        Err(e) => return e,
    };
    let sig = &f.sig;
    if !plain(sig) || !matches!(sig.safety, Safety::Default) || !sig.inputs.is_empty() || !returns_never(&sig.output) {
        return error(Span::call_site(), "`#[entry]` expects `fn() -> !`");
    }
    // Renamed and private: the only way in is the reset handler's `bl main`.
    let inner = format_ident!("__rza1_rt_entry_{}", f.sig.ident);
    f.sig.ident = inner.clone();
    f.vis = syn::Visibility::Inherited;
    quote! {
        #f

        #[doc(hidden)]
        #[unsafe(export_name = "main")]
        pub extern "C" fn __rza1_rt_main() -> ! {
            let f: fn() -> ! = #inner;
            f()
        }
    }
}

fn expand_irq(attr: TokenStream2, item: TokenStream2) -> TokenStream2 {
    let f = match parse(attr, item, "irq") {
        Ok(f) => f,
        Err(e) => return e,
    };
    let sig = &f.sig;
    if !plain(sig) || !matches!(sig.safety, Safety::Unsafe(_)) || !only_input_is_u32(sig) || !matches!(sig.output, ReturnType::Default) {
        return error(Span::call_site(), "`#[irq]` expects `unsafe fn(u32)` (the ICCIAR value)");
    }
    let name = &sig.ident;
    quote! {
        #f

        #[doc(hidden)]
        #[unsafe(export_name = "gic_dispatch")]
        pub unsafe extern "C" fn __rza1_rt_gic_dispatch(__rza1_rt_icciar: u32) {
            let f: unsafe fn(u32) = #name;
            // SAFETY: called once per interrupt by the IRQ entry stub, the caller this
            // function's contract is written for.
            unsafe { f(__rza1_rt_icciar) }
        }
    }
}

fn expand_fault(attr: TokenStream2, item: TokenStream2) -> TokenStream2 {
    let f = match parse(attr, item, "fault") {
        Ok(f) => f,
        Err(e) => return e,
    };
    let sig = &f.sig;
    if !plain(sig) || !matches!(sig.safety, Safety::Default) || !one_typed_input(sig) || !returns_never(&sig.output) {
        return error(Span::call_site(), "`#[fault]` expects `fn(&FaultInfo) -> !`");
    }
    let name = &sig.ident;
    quote! {
        #f

        #[doc(hidden)]
        #[unsafe(export_name = "rza1_cpu_fault_report")]
        pub unsafe extern "C" fn __rza1_rt_fault_report(
            __rza1_rt_pc: u32,
            __rza1_rt_kind: u32,
            __rza1_rt_lr: u32,
            __rza1_rt_sp: u32,
        ) -> ! {
            let f: fn(&::rza1_rt::FaultInfo) -> ! = #name;
            f(&::rza1_rt::FaultInfo::__from_raw(__rza1_rt_pc, __rza1_rt_kind, __rza1_rt_lr, __rza1_rt_sp))
        }
    }
}

#[proc_macro_attribute]
pub fn entry(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_entry(attr.into(), item.into()).into()
}

#[proc_macro_attribute]
pub fn irq(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_irq(attr.into(), item.into()).into()
}

#[proc_macro_attribute]
pub fn fault(attr: TokenStream, item: TokenStream) -> TokenStream {
    expand_fault(attr.into(), item.into()).into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use quote::quote;

    fn squash(ts: TokenStream2) -> String {
        ts.to_string().split_whitespace().collect()
    }

    const ENTRY_ERR: &str = "`#[entry]` expects `fn() -> !`";

    #[test]
    fn entry_exports_main_and_type_checks() {
        let out = squash(expand_entry(quote!(), quote!(fn main() -> ! { loop {} })));
        assert!(out.contains("export_name=\"main\""), "{out}");
        assert!(out.contains("fn()->!"), "{out}");
        assert!(!out.contains("compile_error"), "{out}");
    }

    #[test]
    fn entry_rejects_arguments() {
        let out = expand_entry(quote!(), quote!(fn main(x: u32) -> ! { loop {} })).to_string();
        assert!(out.contains("compile_error") && out.contains(ENTRY_ERR), "{out}");
    }

    #[test]
    fn entry_rejects_a_return_value() {
        let out = expand_entry(quote!(), quote!(fn main() {})).to_string();
        assert!(out.contains("compile_error") && out.contains(ENTRY_ERR), "{out}");
    }

    #[test]
    fn entry_rejects_async() {
        let out = expand_entry(quote!(), quote!(async fn main() -> ! { loop {} })).to_string();
        assert!(out.contains("compile_error") && out.contains(ENTRY_ERR), "{out}");
    }

    #[test]
    fn entry_rejects_unsafe_generic_and_extern() {
        for item in [
            quote!(unsafe fn main() -> ! { loop {} }),
            quote!(fn main<T>() -> ! { loop {} }),
            quote!(extern "C" fn main() -> ! { loop {} }),
        ] {
            let out = expand_entry(quote!(), item).to_string();
            assert!(out.contains("compile_error") && out.contains(ENTRY_ERR), "{out}");
        }
    }

    #[test]
    fn entry_rejects_attribute_arguments() {
        let out = expand_entry(quote!(foo), quote!(fn main() -> ! { loop {} })).to_string();
        assert!(out.contains("compile_error"), "{out}");
    }

    #[test]
    fn irq_exports_gic_dispatch() {
        let out = squash(expand_irq(quote!(), quote!(unsafe fn on_irq(icciar: u32) {})));
        assert!(out.contains("export_name=\"gic_dispatch\""), "{out}");
        assert!(out.contains("extern\"C\""), "{out}");
        assert!(!out.contains("compile_error"), "{out}");
    }

    #[test]
    fn irq_rejects_a_safe_fn() {
        let out = expand_irq(quote!(), quote!(fn on_irq(icciar: u32) {})).to_string();
        assert!(out.contains("compile_error") && out.contains("unsafe fn(u32)"), "{out}");
    }

    #[test]
    fn fault_exports_the_report_symbol() {
        let out = squash(expand_fault(
            quote!(),
            quote!(fn on_fault(info: &rza1_rt::FaultInfo) -> ! { loop {} }),
        ));
        assert!(out.contains("export_name=\"rza1_cpu_fault_report\""), "{out}");
        assert!(out.contains("extern\"C\""), "{out}");
        assert!(!out.contains("compile_error"), "{out}");
    }

    #[test]
    fn fault_rejects_two_parameters() {
        let out = expand_fault(quote!(), quote!(fn on_fault(a: u32, b: u32) -> ! { loop {} })).to_string();
        assert!(out.contains("compile_error") && out.contains("fn(&FaultInfo) -> !"), "{out}");
    }

    #[test]
    fn fault_wrapper_arguments_do_not_shadow_the_handler() {
        // A handler named like one of the reporter's arguments must still be the one called.
        let out = squash(expand_fault(quote!(), quote!(fn pc(info: &rza1_rt::FaultInfo) -> ! { loop {} })));
        assert!(!out.contains("(pc:u32"), "{out}");
        assert!(out.contains("=pc;"), "{out}");
    }

    #[test]
    fn irq_wrapper_argument_does_not_shadow_the_handler() {
        let out = squash(expand_irq(quote!(), quote!(unsafe fn icciar(v: u32) {})));
        assert!(!out.contains("(icciar:u32"), "{out}");
    }

    #[test]
    fn irq_rejects_a_u16_parameter() {
        let out = expand_irq(quote!(), quote!(unsafe fn on_irq(x: u16) {})).to_string();
        assert!(out.contains("compile_error") && out.contains("unsafe fn(u32)"), "{out}");
    }

    #[test]
    fn fault_rejects_a_returning_handler() {
        let out = expand_fault(quote!(), quote!(fn on_fault(info: &rza1_rt::FaultInfo) {})).to_string();
        assert!(out.contains("compile_error") && out.contains("fn(&FaultInfo) -> !"), "{out}");
    }
}
