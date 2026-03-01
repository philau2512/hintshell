@{
    RootModule        = 'HintShellModule.psm1'
    ModuleVersion     = '0.1.0'
    GUID              = '8e6a1c2b-3d4e-5f6a-7b8c-9d0e1f2a3b4c'
    Author            = 'HintShell'
    Description       = '🧠 HintShell - Personal Command Intelligence Engine for PowerShell'
    PowerShellVersion = '7.2'
    FunctionsToExport = @('Start-HintShell', 'Stop-HintShell', 'Get-HintShellStatus', 'Invoke-HSWrapper', 'hs', 'hintshell')
    CmdletsToExport   = @()
    VariablesToExport  = @()
    AliasesToExport    = @()
}
