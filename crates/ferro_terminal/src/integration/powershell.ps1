# FerroIDE shell integration for PowerShell 7 and Windows PowerShell 5.1
# (dot-sourced after the user's profile). Wraps the existing prompt with
# OSC 133 marks: A prompt, B command line, C output, D;<exit code> finished.
if (-not $global:__FerroIntegrated) {
    $global:__FerroIntegrated = $true
    $global:__FerroLastHistoryId = -1
    $global:__FerroUserPrompt = $function:prompt

    function global:prompt {
        # First, before anything resets them.
        $ok = $?
        $nativeCode = $global:LASTEXITCODE
        $e = [char]27
        $bel = [char]7
        $marks = ''
        # Report a finished command only when one ran (not for an empty line).
        $last = Get-History -Count 1
        if ($last -and $last.Id -ne $global:__FerroLastHistoryId) {
            $global:__FerroLastHistoryId = $last.Id
            $code = 1
            if ($ok) {
                $code = 0
            } elseif ($nativeCode -is [int] -and $nativeCode -ne 0) {
                $code = $nativeCode
            }
            $marks += "$e]133;D;$code$bel"
        }
        $marks += "$e]133;A$bel"
        # Written now, so prompts that use Write-Host come after the mark.
        [Console]::Write($marks)
        $text = "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) "
        if ($global:__FerroUserPrompt) {
            $text = (& $global:__FerroUserPrompt) -join ''
        }
        $global:LASTEXITCODE = $nativeCode
        "$text$e]133;B$bel"
    }

    # Mark where the output starts once Enter accepts the line.
    try {
        $accept = 'AcceptLine'
        $current = Get-PSReadLineKeyHandler -Chord Enter -ErrorAction Stop |
            Select-Object -First 1
        if ($current -and $current.Function -eq 'ValidateAndAcceptLine') {
            $accept = 'ValidateAndAcceptLine'
        }
        $global:__FerroAccept = $accept
        Set-PSReadLineKeyHandler -Chord Enter -ScriptBlock {
            if ($global:__FerroAccept -eq 'ValidateAndAcceptLine') {
                [Microsoft.PowerShell.PSConsoleReadLine]::ValidateAndAcceptLine()
            } else {
                [Microsoft.PowerShell.PSConsoleReadLine]::AcceptLine()
            }
            [Console]::Write("$([char]27)]133;C$([char]7)")
        }
    } catch {
        # No PSReadLine: prompts and exit codes still work.
    }
}
