# FerroIDE shell integration for zsh (login shells): the user's .zprofile.
if [[ -r "$__ferro_user_zdotdir/.zprofile" ]]; then
    ZDOTDIR="$__ferro_user_zdotdir"
    . "$__ferro_user_zdotdir/.zprofile"
    ZDOTDIR="$__ferro_zdotdir"
fi
