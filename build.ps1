# Сборка релизного pglistener.exe с очисткой артефактов.
#
# 1. cargo build --release
# 2. копирует target\release\pglistener.exe в папку -OutDir
#    (по умолчанию — папка над исходниками, где лежат pglistener.exe и pglistener.db);
# 3. только если сборка и копирование прошли успешно, удаляет папку target (cargo clean).
#
# При ошибке target остаётся на месте, чтобы следующая сборка не начиналась с нуля.
#
# Запуск: .\build.ps1 [-OutDir <папка>] [-KeepTarget]
#   -KeepTarget  не удалять target после сборки.

param(
    [string]$OutDir = (Split-Path -Parent $PSScriptRoot),
    [switch]$KeepTarget
)

$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

function Fail([string]$message) {
    Write-Host "ОШИБКА: $message" -ForegroundColor Red
    exit 1
}

# cargo: из PATH, иначе из CARGO_HOME, иначе из тулчейна рядом с проектом (..\..\tools).
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    $candidates = @()
    if ($env:CARGO_HOME) { $candidates += $env:CARGO_HOME }
    $candidates += (Join-Path $PSScriptRoot '..\..\tools\cargo')
    foreach ($home_ in $candidates) {
        $bin = Join-Path $home_ 'bin'
        if (Test-Path (Join-Path $bin 'cargo.exe')) {
            $env:CARGO_HOME = (Resolve-Path $home_).Path
            $rustup = Join-Path (Split-Path -Parent $env:CARGO_HOME) 'rustup'
            if (-not $env:RUSTUP_HOME -and (Test-Path $rustup)) { $env:RUSTUP_HOME = (Resolve-Path $rustup).Path }
            $env:PATH = "$((Resolve-Path $bin).Path);$env:PATH"
            break
        }
    }
    if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
        Fail 'cargo не найден. Установите Rust (https://rustup.rs) или задайте CARGO_HOME.'
    }
}

$started = Get-Date
Write-Host '==> cargo build --release' -ForegroundColor Cyan
& cargo build --release
if ($LASTEXITCODE -ne 0) { Fail "сборка не удалась (код $LASTEXITCODE), папка target сохранена." }

$built = Join-Path $PSScriptRoot 'target\release\pglistener.exe'
if (-not (Test-Path $built)) { Fail "после сборки нет файла $built" }

if (-not (Test-Path $OutDir)) { New-Item -ItemType Directory -Path $OutDir | Out-Null }
$dest = Join-Path (Resolve-Path $OutDir).Path 'pglistener.exe'
Write-Host "==> копирование в $dest" -ForegroundColor Cyan
try {
    Copy-Item -LiteralPath $built -Destination $dest -Force
} catch {
    Fail "не удалось заменить $dest (возможно, программа запущена): $($_.Exception.Message). Собранный файл остался в $built"
}
if ((Get-FileHash $built).Hash -ne (Get-FileHash $dest).Hash) {
    Fail "скопированный файл не совпадает с собранным, папка target сохранена."
}

if ($KeepTarget) {
    Write-Host '==> -KeepTarget: папка target сохранена' -ForegroundColor Yellow
} else {
    Write-Host '==> cargo clean' -ForegroundColor Cyan
    & cargo clean
    if ($LASTEXITCODE -ne 0) { Fail "cargo clean завершился с кодом $LASTEXITCODE" }
}

$size = [math]::Round((Get-Item $dest).Length / 1MB, 1)
$minutes = [math]::Round(((Get-Date) - $started).TotalMinutes, 1)
Write-Host "Готово: $dest ($size МБ) за $minutes мин." -ForegroundColor Green
