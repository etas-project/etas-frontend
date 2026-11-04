flow render_prompt_prefix(role: string) -> string { return "role:" + role; }
flow main() -> string {
    let planning = render_prompt_prefix("reviewer");
    let execution = render_prompt_prefix("reviewer");
    return planning + execution;
}
