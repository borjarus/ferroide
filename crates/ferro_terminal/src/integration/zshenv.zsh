# FerroIDE shell integration for zsh: this directory is ZDOTDIR, so zsh
# reads these files; each one reads the user's own file first.
__ferro_zdotdir="$ZDOTDIR"
__ferro_user_zdotdir="${FERRO_USER_ZDOTDIR:-$HOME}"
unset FERRO_USER_ZDOTDIR
if [[ -r "$__ferro_user_zdotdir/.zshenv" ]]; then
    ZDOTDIR="$__ferro_user_zdotdir"
    . "$__ferro_user_zdotdir/.zshenv"
    # The user's .zshenv may move ZDOTDIR itself.
    __ferro_user_zdotdir="$ZDOTDIR"
    ZDOTDIR="$__ferro_zdotdir"
fi
