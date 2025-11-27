# https://just.systems

set windows-shell := ["pwsh.exe", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
set shell := ["pwsh", "-NoLogo", "-NoProfile", "-NonInteractive", "-Command"]
#set script-interpreter := ["pwsh", "-NoLogo", "-NoProfile", "-NonInteractive"]


mutants:
    cargo mutants \
        --no-shuffle \
        --baseline=skip \
        --test-workspace=true \
        --colors=never \
        --jobs=4 \
        --build-timeout=600 \
        --timeout=300 \
        -vV
