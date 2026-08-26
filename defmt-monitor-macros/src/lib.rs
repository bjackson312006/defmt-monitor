//! Proc macro implementation for [`defmt-monitor`]. Use that crate, not this one.
//!
//! [`defmt-monitor`]: https://docs.rs/defmt-monitor

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, LitStr, Token};

/// Prefix identifying a monitor frame, and the version of the wire format.
///
/// Must match `defmt_monitor::SENTINEL`.
const SENTINEL: &str = "[MON2]";

/// Compile-time switch. Unset means enabled.
const ENABLE_VAR: &str = "DEFMT_MONITOR";

/// Whether `monitor!` should emit anything.
///
/// Defaults to on, so that adding a `monitor!` call to a fresh project produces data
/// without first having to discover an environment variable.
fn enabled() -> bool {
    match std::env::var(ENABLE_VAR) {
        Ok(value) => !matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "off" | "0" | "false" | "no"
        ),
        Err(_) => true,
    }
}

struct MonitorInput {
    topic: LitStr,
    /// Optional `desc = "..."` written straight after the topic.
    description: Option<LitStr>,
    spec: LitStr,
    args: Punctuated<Expr, Token![,]>,
}

impl Parse for MonitorInput {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let topic = input.parse::<LitStr>().map_err(|e| {
            syn::Error::new(e.span(), "expected a string literal topic, e.g. \"imu/accel/x\"")
        })?;
        input.parse::<Token![,]>()?;

        // `desc = "..."` is optional, so it is only consumed when actually present.
        let description = if input.peek(syn::Ident) && input.peek2(Token![=]) {
            let name = input.parse::<syn::Ident>()?;
            if name != "desc" {
                return Err(syn::Error::new(
                    name.span(),
                    format!("expected `desc = \"...\"`, found `{name}`"),
                ));
            }
            input.parse::<Token![=]>()?;
            let value = input.parse::<LitStr>().map_err(|e| {
                syn::Error::new(e.span(), "a monitor description must be a string literal")
            })?;
            input.parse::<Token![,]>()?;
            Some(value)
        } else {
            None
        };

        let spec = input.parse::<LitStr>().map_err(|e| {
            syn::Error::new(e.span(), "expected a defmt format string literal, e.g. \"{=f32}\"")
        })?;

        let args = if input.peek(Token![,]) {
            input.parse::<Token![,]>()?;
            Punctuated::parse_terminated(input)?
        } else {
            Punctuated::new()
        };

        Ok(MonitorInput {
            topic,
            description,
            spec,
            args,
        })
    }
}

/// Implementation of `defmt_monitor::monitor!`.
#[proc_macro]
pub fn monitor(input: TokenStream) -> TokenStream {
    let MonitorInput {
        topic,
        description,
        spec,
        args,
    } = syn::parse_macro_input!(input as MonitorInput);

    let topic_value = topic.value();
    if let Err(msg) = validate_field(&topic_value, "topic") {
        return syn::Error::new(topic.span(), msg).to_compile_error().into();
    }

    let description_value = description.as_ref().map(LitStr::value).unwrap_or_default();
    if let Some(literal) = &description
        && let Err(msg) = validate_field(&description_value, "description")
    {
        return syn::Error::new(literal.span(), msg).to_compile_error().into();
    }

    // Build the defmt format string here so that defmt's own proc macro receives a
    // genuine string literal token. Topic and description are therefore interned into
    // the ELF's `.defmt` section at compile time and cost zero bytes on the wire, however
    // long they are. An omitted description leaves an empty slot rather than no slot, so
    // the layout is fixed and needs no guessing to parse.
    let fmt = LitStr::new(
        &format!(
            "{SENTINEL}[{topic_value}][{description_value}][{}]",
            spec.value()
        ),
        spec.span(),
    );

    let args: Vec<&Expr> = args.iter().collect();

    // Cargo does not track environment variables read by a proc macro, so the switch
    // is also expanded into the generated code where *rustc* records it. Without this,
    // flipping `DEFMT_MONITOR` would not trigger a rebuild.
    let tracked = quote!(option_env!("DEFMT_MONITOR"););

    let expanded = if enabled() {
        // `println!` rather than `info!`: monitor samples are not a log level, and
        // `println!` carries no level tag, so `DEFMT_LOG` cannot filter them away.
        // It shares `info!`'s codegen otherwise, timestamp included.
        //
        // `defmt`'s macros expand to bare `defmt::export::*` paths, so they resolve
        // against the *calling* crate's `defmt` dependency. Emitting `::defmt::` here
        // keeps this crate agnostic to which defmt version the firmware uses.
        quote!({
            #tracked
            ::defmt::println!(#fmt #(, #args)*)
        })
    } else {
        // Disabled: nothing is emitted and no string is interned into the ELF. The
        // arguments are still name-resolved and type-checked so they cannot rot while
        // monitoring is off, but the branch is never taken and optimises away.
        quote!({
            #tracked
            if false {
                let _ = (#(&#args,)*);
            }
        })
    };
    expanded.into()
}

/// Rejects topics and descriptions that would make the emitted format string ambiguous
/// to parse, or that defmt would misinterpret as containing format arguments.
///
/// Only the value, which comes last, may contain brackets.
fn validate_field(value: &str, field: &str) -> Result<(), String> {
    if field == "topic" && value.is_empty() {
        return Err("monitor topic must not be empty".to_string());
    }
    for c in ['[', ']', '{', '}'] {
        if value.contains(c) {
            return Err(format!(
                "monitor {field} must not contain `{c}`; it would make the frame ambiguous \
                 to the host parser."
            ));
        }
    }
    Ok(())
}
