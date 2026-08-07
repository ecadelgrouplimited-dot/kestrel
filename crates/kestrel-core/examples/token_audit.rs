//! Throwaway: measure the static per-turn token footprint (system prompt + tool
//! schemas) for each profile — the bytes resent on every request.
use kestrel_core::{estimate_tokens, system_prompt_for, tools_for, Profile};

fn main() {
    let root = std::path::Path::new(".");
    for (name, profile) in [
        ("Build", Profile::Build),
        ("Work", Profile::Work),
        ("Motion", Profile::Motion),
    ] {
        let system = system_prompt_for(profile, root);
        let tools = tools_for(profile);
        let tools_json = serde_json::to_string(
            &tools
                .iter()
                .map(|t| {
                    serde_json::json!({
                        "name": t.name,
                        "description": t.description,
                        "parameters": t.input_schema,
                    })
                })
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let sys_tok = estimate_tokens(system.len());
        let tools_tok = estimate_tokens(tools_json.len());
        println!(
            "{name:7} | {n:2} tools | system {sys_ch:>5}ch ~{sys_tok:>4}t | tools {tools_ch:>6}ch ~{tools_tok:>5}t | static ~{tot}t/turn",
            n = tools.len(),
            sys_ch = system.len(),
            tools_ch = tools_json.len(),
            tot = sys_tok + tools_tok,
        );
    }

    // The fattest individual tool descriptions (top offenders to trim).
    let mut all: Vec<(String, usize)> = tools_for(Profile::Build)
        .into_iter()
        .chain(tools_for(Profile::Motion))
        .map(|t| {
            let bytes = serde_json::to_string(&serde_json::json!({
                "name": t.name, "description": t.description, "parameters": t.input_schema,
            }))
            .unwrap()
            .len();
            (t.name, bytes)
        })
        .collect();
    all.sort_by_key(|(_, b)| std::cmp::Reverse(*b));
    all.dedup_by_key(|(n, _)| n.clone());
    println!("\nFattest tools (name: ~tokens):");
    for (name, bytes) in all.into_iter().take(15) {
        println!("  {name:20} ~{}t", estimate_tokens(bytes));
    }
}
