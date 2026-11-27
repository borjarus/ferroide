# FerroIDE shell integration for zsh: the user's .zshrc, then OSC 133 marks
# (A prompt, B command line, C output, D;<exit code> finished).
if [[ -r "$__ferro_user_zdotdir/.zshrc" ]]; then
    ZDOTDIR="$__ferro_user_zdotdir"
    . "$__ferro_user_zdotdir/.zshrc"
fi
# From here on zsh reads the user's files (.zlogin, .zlogout).
ZDOTDIR="$__ferro_user_zdotdir"
unset __ferro_zdotdir __ferro_user_zdotdir

if [[ -z "$__ferro_integrated" ]]; then
    __ferro_integrated=1
    typeset -g __ferro_ran=0

    # First precmd hook: report how the command ended.
    __ferro_precmd() {
        local code=$?
        if (( __ferro_ran )); then
            printf '\033]133;D;%s\007' "$code"
        fi
        __ferro_ran=0
        return $code
    }

    # Last precmd hook: mark the prompt, even when a theme rebuilt it.
    __ferro_wrap_prompt() {
        if [[ "$PS1" != *'133;A'* ]]; then
            PS1=$'%{\e]133;A\a%}'"$PS1"$'%{\e]133;B\a%}'
        fi
    }

    __ferro_preexec() {
        __ferro_ran=1
        printf '\033]133;C\007'
    }

    precmd_functions=(__ferro_precmd $precmd_functions __ferro_wrap_prompt)
    preexec_functions+=(__ferro_preexec)
fi
