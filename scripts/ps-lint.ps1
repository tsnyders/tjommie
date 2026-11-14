# A cheap static check for the PowerShell scripts that cannot be run where they are written (the installer acceptance scripts run only on
# a GitHub-hosted runner): with the library loaded, every variable a script reads is assigned in it, is a parameter, or is an
# automatic variable; and every command it calls is a cmdlet, or a function defined in it or in a library it loads. A misspelled name
# otherwise fails only when the branch that uses it runs, on the runner. Prints JSON: { file: [problems] }.
#
# Parsing and Get-Command only; nothing in the files is run. The libraries are dot-sourced, which defines functions and nothing else.
# -Files is one comma-separated string: -File cannot pass an array.
param([Parameter(Mandatory)][string]$Installer, [Parameter(Mandatory)][string]$Files)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
. (Join-Path $Installer 'hosted-runner.ps1')
. (Join-Path $Installer 'accept-lib.ps1')

$automatic = @('_', 'PSItem', 'PSScriptRoot', 'PSCommandPath', 'PSVersionTable', 'PID', 'true', 'false', 'null', 'args', 'input', 'Error', 'matches',
    'LASTEXITCODE', 'ErrorActionPreference', 'ProgressPreference', 'MyInvocation', 'PSBoundParameters', 'foreach', 'this', 'IsWindows', 'Host', 'PSEdition')

function Get-Unqualified([string]$Path) { return ($Path -replace '^(script|global|local|private):', '') }

$report = [ordered]@{}
foreach ($name in ($Files -split ',')) {
    $path = Join-Path $Installer $name
    $tokens = $null
    $errors = $null
    $ast = [System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$tokens, [ref]$errors)
    $problems = @()
    foreach ($error_ in $errors) { $problems += "parse error at line $($error_.Extent.StartLineNumber): $($error_.Message)" }

    $functions = @($ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.FunctionDefinitionAst] }, $true) | ForEach-Object { $_.Name })
    $assigned = New-Object 'System.Collections.Generic.HashSet[string]' ([StringComparer]::OrdinalIgnoreCase)
    foreach ($parameter in $ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.ParameterAst] }, $true)) { [void]$assigned.Add((Get-Unqualified $parameter.Name.VariablePath.UserPath)) }
    foreach ($assignment in $ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.AssignmentStatementAst] }, $true)) {
        $left = $assignment.Left
        while ($left -is [System.Management.Automation.Language.ConvertExpressionAst]) { $left = $left.Child }
        if ($left -is [System.Management.Automation.Language.VariableExpressionAst]) { [void]$assigned.Add((Get-Unqualified $left.VariablePath.UserPath)) }
    }
    foreach ($loop in $ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.ForEachStatementAst] }, $true)) { [void]$assigned.Add((Get-Unqualified $loop.Variable.VariablePath.UserPath)) }
    # Variables the library assigns at its top level and the script may read as $script:name are the library's own business.
    foreach ($variable in $ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.VariableExpressionAst] }, $true)) {
        $variablePath = $variable.VariablePath
        if ($variablePath.IsDriveQualified) { continue }
        $unqualified = Get-Unqualified $variablePath.UserPath
        if ($automatic -contains $unqualified -or $assigned.Contains($unqualified)) { continue }
        $problems += "line $($variable.Extent.StartLineNumber): `$$unqualified is read but never assigned"
    }

    foreach ($command in $ast.FindAll({ param($node) $node -is [System.Management.Automation.Language.CommandAst] }, $true)) {
        $commandName = $command.GetCommandName()
        if (-not $commandName -or $functions -contains $commandName) { continue }
        if (Get-Command -Name $commandName -ErrorAction SilentlyContinue) { continue }
        $problems += "line $($command.Extent.StartLineNumber): command $commandName is not defined"
    }
    $report[$name] = @($problems | Select-Object -Unique)
}
$report | ConvertTo-Json -Compress -Depth 4
