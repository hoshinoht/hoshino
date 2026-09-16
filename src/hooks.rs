use crate::cli::Shell;

/// Generate a sourceable hook integration for the selected shell.
pub fn generate(shell: Shell) -> String {
    match shell {
        Shell::Bash => bash(),
        Shell::Zsh => zsh(),
        Shell::Fish => fish(),
        Shell::Nu => nushell(),
        Shell::Powershell => powershell(),
    }
    .to_owned()
}

fn bash() -> &'static str {
    r#"if [[ -z ${__HOSHINO_HOOKS_INSTALLED-} ]]; then
    __HOSHINO_HOOKS_INSTALLED=1
    __HOSHINO_HOOK_LAST_PWD=

    __hoshino_hook_emit() {
        if [[ ${__HOSHINO_HOOK_LAST_PWD-} == "$PWD" ]]; then
            return 0
        fi
        __HOSHINO_HOOK_LAST_PWD="$PWD"
        command hoshino --hook-context || :
    }

    if [[ "$(declare -p PROMPT_COMMAND 2>/dev/null || :)" == "declare -a PROMPT_COMMAND="* ]]; then
        PROMPT_COMMAND+=(__hoshino_hook_emit)
    elif [[ -n ${PROMPT_COMMAND-} ]]; then
        PROMPT_COMMAND="${PROMPT_COMMAND}; __hoshino_hook_emit"
    else
        PROMPT_COMMAND=__hoshino_hook_emit
    fi
fi
"#
}

fn zsh() -> &'static str {
    r#"if [[ -z ${__HOSHINO_HOOKS_INSTALLED-} ]]; then
    typeset -g __HOSHINO_HOOKS_INSTALLED=1
    typeset -g __HOSHINO_HOOK_LAST_PWD=

    __hoshino_hook_emit() {
        if [[ ${__HOSHINO_HOOK_LAST_PWD-} == "$PWD" ]]; then
            return 0
        fi
        __HOSHINO_HOOK_LAST_PWD="$PWD"
        command hoshino --hook-context || :
    }

    autoload -Uz add-zsh-hook
    add-zsh-hook precmd __hoshino_hook_emit
    add-zsh-hook chpwd __hoshino_hook_emit
fi
"#
}

fn fish() -> &'static str {
    r#"if not set -q __HOSHINO_HOOKS_INSTALLED
    set -g __HOSHINO_HOOKS_INSTALLED 1
    set -g __HOSHINO_HOOK_LAST_PWD ""

    function __hoshino_hook_emit
        if test "$__HOSHINO_HOOK_LAST_PWD" = "$PWD"
            return
        end
        set -g __HOSHINO_HOOK_LAST_PWD "$PWD"
        command hoshino --hook-context
    end

    function __hoshino_hook_prompt --on-event fish_prompt
        __hoshino_hook_emit
    end

    function __hoshino_hook_pwd --on-variable PWD
        __hoshino_hook_emit
    end
end
"#
}

fn nushell() -> &'static str {
    r#"def --env __hoshino_hook_emit [] {
    let current = ($env.PWD | into string)
    let previous = ($env.__HOSHINO_HOOK_LAST_PWD? | default "")
    if $previous == $current {
        return
    }
    $env.__HOSHINO_HOOK_LAST_PWD = $current
    ^hoshino --hook-context
}

let __hoshino_hooks_installed = ($env.__HOSHINO_HOOKS_INSTALLED? | default false)
if $__hoshino_hooks_installed == false {
    $env.__HOSHINO_HOOKS_INSTALLED = true

    let __hoshino_pre_hook = {|| __hoshino_hook_emit }
    let __hoshino_existing_pre_prompt = ($env.config.hooks.pre_prompt? | default null)
    let __hoshino_pre_prompt = (
        if $__hoshino_existing_pre_prompt == null {
            [$__hoshino_pre_hook]
        } else if ($__hoshino_existing_pre_prompt | describe | str starts-with "list") {
            $__hoshino_existing_pre_prompt | append $__hoshino_pre_hook
        } else {
            [$__hoshino_existing_pre_prompt, $__hoshino_pre_hook]
        }
    )

    let __hoshino_existing_env_change = ($env.config.hooks.env_change? | default {})
    let __hoshino_existing_pwd = ($__hoshino_existing_env_change.PWD? | default null)
    let __hoshino_pwd_hook = {|before, after| __hoshino_hook_emit }
    let __hoshino_pwd = (
        if $__hoshino_existing_pwd == null {
            [$__hoshino_pwd_hook]
        } else if ($__hoshino_existing_pwd | describe | str starts-with "list") {
            $__hoshino_existing_pwd | append $__hoshino_pwd_hook
        } else {
            [$__hoshino_existing_pwd, $__hoshino_pwd_hook]
        }
    )

    let __hoshino_env_change = ($__hoshino_existing_env_change | upsert PWD $__hoshino_pwd)
    $env.config = (
        $env.config
        | upsert hooks.pre_prompt $__hoshino_pre_prompt
        | upsert hooks.env_change $__hoshino_env_change
    )
end
"#
}

fn powershell() -> &'static str {
    r#"if (-not (Get-Variable -Name __HoshinoHooksInstalled -Scope Global -ErrorAction SilentlyContinue)) {
    Set-Variable -Name __HoshinoHooksInstalled -Scope Global -Value $true
    Set-Variable -Name __HoshinoHookLastPwd -Scope Global -Value $null
    Set-Variable -Name __HoshinoOriginalPrompt -Scope Global -Value ((Get-Command prompt -CommandType Function).ScriptBlock)

    function global:__HoshinoHookEmit {
        $current = (Get-Location).Path
        $previous = Get-Variable -Name __HoshinoHookLastPwd -Scope Global -ValueOnly -ErrorAction SilentlyContinue
        if ($previous -eq $current) {
            return
        }
        Set-Variable -Name __HoshinoHookLastPwd -Scope Global -Value $current
        hoshino --hook-context
    }

    function global:prompt {
        __HoshinoHookEmit
        $originalResult = (Get-Variable -Name __HoshinoOriginalPrompt -Scope Global -ValueOnly).Invoke()
        $originalResult
    }
}
"#
}
