# FerroIDE shell integration for fish (sourced by --init-command after the
# user's configuration): OSC 133 marks (A prompt, B command line, C output,
# D;<exit code> finished).
if not set -q __ferro_integrated
    set -g __ferro_integrated 1

    function __ferro_preexec --on-event fish_preexec
        printf '\e]133;C\a'
    end

    function __ferro_postexec --on-event fish_postexec
        printf '\e]133;D;%s\a' $status
    end

    if functions -q fish_prompt
        functions -c fish_prompt __ferro_user_prompt
    else
        function __ferro_user_prompt
            printf '%s> ' (prompt_pwd)
        end
    end

    function __ferro_set_status
        return $argv[1]
    end

    function fish_prompt
        set -l code $status
        printf '\e]133;A\a'
        # Give the user's prompt the status it would have seen.
        __ferro_set_status $code
        __ferro_user_prompt
        printf '\e]133;B\a'
    end
end
