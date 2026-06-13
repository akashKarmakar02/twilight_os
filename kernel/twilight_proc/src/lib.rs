#![feature(proc_macro_diagnostic, proc_macro_span)]

mod cpu_local;

#[macro_use]
extern crate proc_macro_error;

use proc_macro::TokenStream;

#[proc_macro_attribute]
#[proc_macro_error]
pub fn cpu_local(attr: TokenStream, item: TokenStream) -> TokenStream {
    cpu_local::parse(attr, item)
}
