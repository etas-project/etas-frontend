flow compile_system_prompt(role: string) -> string { return "system:" + role; }
flow main() -> string {
    var prompts = "";
    for _ in [1, 2, 3, 4] limit Iterations(4) {
        let system = compile_system_prompt("reviewer");
        prompts = prompts + system;
    }
    return prompts;
}
