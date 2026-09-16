use std::{env, fs, process::Command as ProcessCommand};

use hoshino::{cli::Shell, hooks::generate};

const SHELLS: [Shell; 5] = [
    Shell::Bash,
    Shell::Zsh,
    Shell::Fish,
    Shell::Nu,
    Shell::Powershell,
];

#[test]
fn every_shell_has_one_guarded_fixed_invocation() {
    for shell in SHELLS {
        let source = generate(shell);
        let lower = source.to_ascii_lowercase();

        assert!(!source.is_empty(), "{shell:?} generator is empty");
        assert!(lower.contains("hoshino"), "{shell:?} has no namespace");
        assert!(
            lower.contains("installed"),
            "{shell:?} has no install guard"
        );
        assert!(
            source.contains("HOSHINO_HOOK_LAST_PWD") || source.contains("HoshinoHookLastPwd"),
            "{shell:?} has no same-directory state"
        );
        assert_eq!(
            source.matches("hoshino --hook-context").count(),
            1,
            "{shell:?} must have one invocation site"
        );
        assert!(
            !lower.contains("git"),
            "{shell:?} parses Git state in shell code"
        );
        assert!(
            !lower.contains("mew"),
            "{shell:?} contains an upstream name"
        );
        assert!(
            !lower.contains("eval"),
            "{shell:?} evaluates generated data"
        );
        assert!(!source.contains('&'), "{shell:?} starts a background job");
    }
}

#[test]
fn bash_composes_prompt_command_string_and_array_forms() {
    let source = generate(Shell::Bash);

    assert!(source.contains("declare -p PROMPT_COMMAND"));
    assert!(source.contains("PROMPT_COMMAND+=(__hoshino_hook_emit)"));
    assert!(source.contains("PROMPT_COMMAND=\"${PROMPT_COMMAND}; __hoshino_hook_emit\""));
    assert!(source.contains("PROMPT_COMMAND=__hoshino_hook_emit"));
    assert!(source.contains("__HOSHINO_HOOKS_INSTALLED"));
}

#[test]
fn zsh_uses_add_zsh_hook_without_replacing_existing_handlers() {
    let source = generate(Shell::Zsh);

    assert!(source.contains("autoload -Uz add-zsh-hook"));
    assert!(source.contains("add-zsh-hook precmd __hoshino_hook_emit"));
    assert!(source.contains("add-zsh-hook chpwd __hoshino_hook_emit"));
}

#[test]
fn fish_uses_named_prompt_and_pwd_events() {
    let source = generate(Shell::Fish);

    assert!(source.contains("--on-event fish_prompt"));
    assert!(source.contains("--on-variable PWD"));
    assert!(source.contains("function __hoshino_hook_prompt"));
    assert!(source.contains("function __hoshino_hook_pwd"));
    assert!(!source.contains("function fish_prompt"));
}

#[test]
fn nushell_merges_both_existing_hook_slots() {
    let source = generate(Shell::Nu);

    assert!(source.contains("def --env __hoshino_hook_emit"));
    assert!(source.contains("upsert hooks.pre_prompt"));
    assert!(source.contains("upsert hooks.env_change"));
    assert!(source.contains("__hoshino_existing_pre_prompt | append"));
    assert!(source.contains("__hoshino_existing_pwd | append"));
    assert!(source.contains("PWD"));
}

#[test]
fn powershell_saves_and_returns_the_prior_prompt_result() {
    let source = generate(Shell::Powershell);

    assert!(source.contains("Get-Command prompt -CommandType Function"));
    assert!(source.contains("__HoshinoOriginalPrompt"));
    assert!(source.contains("function global:prompt"));
    assert!(source.contains("__HoshinoHookEmit"));
    assert!(source.contains(".Invoke()"));
    assert!(source.contains("$originalResult"));
}

#[test]
fn installed_posix_parsers_accept_generated_sources() {
    let parsers = [
        (Shell::Bash, "bash", ["-n"].as_slice()),
        (Shell::Zsh, "zsh", ["-n"].as_slice()),
        (Shell::Fish, "fish", ["-n"].as_slice()),
    ];

    for (shell, executable, flags) in parsers {
        let available = ProcessCommand::new(executable)
            .arg("--version")
            .status()
            .is_ok_and(|status| status.success());
        if !available {
            continue;
        }

        let path = env::temp_dir().join(format!(
            "hoshino-hooks-{}-{}",
            executable,
            std::process::id()
        ));
        fs::write(&path, generate(shell)).expect("write generated shell source");
        let status = ProcessCommand::new(executable)
            .args(flags)
            .arg(&path)
            .status()
            .expect("run installed shell parser");
        fs::remove_file(&path).expect("remove generated shell source");
        assert!(status.success(), "{executable} rejected generated source");
    }
}
