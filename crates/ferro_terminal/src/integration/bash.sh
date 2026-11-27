# FerroIDE shell integration for bash (loaded with --rcfile).
# Marks prompts and commands with OSC 133 (A prompt, B command line,
# C output, D;<exit code> finished) and leaves the user's setup alone.

if [ -n "$FERRO_BASH_LOGIN" ]; then
    # --rcfile does not combine with --login: read what a login shell reads.
    unset FERRO_BASH_LOGIN
    [ -r /etc/profile ] && . /etc/profile
    if [ -r ~/.bash_profile ]; then
        . ~/.bash_profile
    elif [ -r ~/.bash_login ]; then
        . ~/.bash_login
    elif [ -r ~/.profile ]; then
        . ~/.profile
    fi
else
    [ -r ~/.bashrc ] && . ~/.bashrc
fi

if [ -z "$__ferro_integrated" ]; then
    __ferro_integrated=1
    __ferro_ran=0
    __ferro_x=

    # First in PROMPT_COMMAND: report how the command ended.
    __ferro_precmd() {
        local code=$?
        if [ "$__ferro_ran" = 1 ]; then
            printf '\033]133;D;%s\007' "$code"
        fi
        __ferro_ran=0
        return $code
    }

    # Last in PROMPT_COMMAND: mark the prompt, even when a theme rebuilt it.
    __ferro_wrap_ps1() {
        local code=$?
        case "$PS1" in
            *'133;A'*) ;;
            *) PS1="\[\033]133;A\007\]$PS1\[\033]133;B\007\]" ;;
        esac
        return $code
    }

    if [ "$(declare -p PROMPT_COMMAND 2>/dev/null | cut -c1-10)" = "declare -a" ]; then
        PROMPT_COMMAND=(__ferro_precmd "${PROMPT_COMMAND[@]}" __ferro_wrap_ps1)
    else
        # Newlines, not `;`: the user's value may end with one already.
        PROMPT_COMMAND="__ferro_precmd"$'\n'"${PROMPT_COMMAND}"$'\n'"__ferro_wrap_ps1"
    fi

    # PS0 is shown when a command line is accepted (bash 4.4+). The
    # arithmetic in the empty substring sets the flag in this shell.
    PS0="${PS0}\${__ferro_x:\$((__ferro_ran=1)):0}\033]133;C\007"
fi
