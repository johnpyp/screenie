# The demo desktop's shell.
HISTFILE=/dev/null
PS1='\[\e[38;2;203;166;247m\]'$'\uf306'' \[\e[38;2;137;180;250m\]\w \[\e[38;2;166;227;161m\]❯\[\e[0m\] '
alias ls='eza --icons'
alias cat='batcat --paging=never --style=plain --theme=ansi'
cd ~/code/acme-api 2>/dev/null
