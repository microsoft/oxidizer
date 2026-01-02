# Check the spelling of the workspace contents.
[script]
spellcheck:
    $ErrorActionPreference = "Stop"
    $PSNativeCommandUseErrorActionPreference = $true

    # The `.spelling` file we have is not strictly speaking `cargo spellcheck` compatible because
    # it has very strict requirements that do not play well with Code Spell Checker in the IDE.
    # Therefore, we preprocess the `.spelling` file into `target/spelling.dic` before executing.
    #
    # What do we have to do?
    # 1. Sort lines alphabetically.
    # 2. Remove lines that consist solely of numbers.
    # 3. Prepend a line containing the count of (remaining) lines.
    $spelling_file = ".spelling"
    $output_file = "target/spelling.dic"

    $lines = Get-Content $spelling_file | Sort-Object
    $filtered_lines = $lines | Where-Object { $_ -notmatch '^\d+$' }
    $line_count = $filtered_lines.Count

    $output_content = @($line_count) + $filtered_lines
    $output_content | Set-Content $output_file

    cargo spellcheck --cfg spellcheck.toml --code 1
