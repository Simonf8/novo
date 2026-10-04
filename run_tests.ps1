param (
    [string]$TestsDir = "tests"
)

if (-Not (Test-Path $TestsDir)) {
    Write-Host "Tests directory '$TestsDir' not found." -ForegroundColor Red
    exit 1
}

Write-Host "Building novo release..."
$pyPath = python -c "import sys; print(sys.base_exec_prefix)"
$env:PATH = "$pyPath;$env:PATH"
$env:PYTHONPATH = "C:\Users\kefet\AppData\Local\hermes\hermes-agent\venv\Lib\site-packages"

& "$env:USERPROFILE\.cargo\bin\cargo" build --release
if ($LASTEXITCODE -ne 0) {
    Write-Host "Build failed!" -ForegroundColor Red
    exit 1
}

$testFiles = Get-ChildItem -Path $TestsDir -Filter *.nv
$passed = 0
$failed = 0

foreach ($file in $testFiles) {
    Write-Host "Testing $($file.Name)..." -NoNewline
    
    $astOutput = & .\target\release\novo.exe $file.FullName --engine=ast 2>&1
    $astExitCode = $LASTEXITCODE
    
    $vmOutput = & .\target\release\novo.exe $file.FullName --engine=vm 2>&1
    $vmExitCode = $LASTEXITCODE
    
    $astOutputStr = ($astOutput | Out-String).Trim()
    $vmOutputStr = ($vmOutput | Out-String).Trim()
    
    if ($astExitCode -ne $vmExitCode) {
        Write-Host " FAILED (Exit Code Mismatch: AST=$astExitCode, VM=$vmExitCode)" -ForegroundColor Red
        $failed++
    } elseif ($astOutputStr -cne $vmOutputStr) {
        Write-Host " FAILED (Output Mismatch)" -ForegroundColor Red
        Write-Host "--- AST Output ---" -ForegroundColor DarkGray
        Write-Host $astOutputStr
        Write-Host "--- VM Output ---" -ForegroundColor DarkGray
        Write-Host $vmOutputStr
        Write-Host "------------------" -ForegroundColor DarkGray
        $failed++
    } else {
        Write-Host " PASSED" -ForegroundColor Green
        $passed++
    }
}

Write-Host ""
Write-Host "Differential Testing Complete!"
Write-Host "Passed: $passed" -ForegroundColor Green
if ($failed -gt 0) {
    Write-Host "Failed: $failed" -ForegroundColor Red
    exit 1
} else {
    exit 0
}
