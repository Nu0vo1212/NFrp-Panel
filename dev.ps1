# =============================================================================
#  FrpPanel 开发启动器（Windows PowerShell 5.1+）
#
#  通常由根目录的 dev.bat 调用，也可以直接：
#      powershell -ExecutionPolicy Bypass -File .\dev.ps1
#
#  模式：
#      dev       后端(cargo run) + 前端(Vite HMR)，默认
#      build     构建前端，然后由后端单进程托管 dist
#      backend   只启动后端
#      frontend  只启动前端（要求后端已运行）
#      status    查看端口 / 健康状态 / PID
#      stop      停止 dev.bat 启动的进程
#      help      帮助
#
#  选项：
#      --port N        后端端口（默认 8787）
#      --ui-port N     Vite 端口（默认 5173）
#      --release       后端使用 release 构建
#      --fresh         使用隔离数据目录 data/dev，首次口令 paneladmin123
#      --restart       启动前停止本启动器记录的旧进程
#      --no-open       不自动打开浏览器
#      --no-install    前端依赖缺失时不自动 npm install
# =============================================================================

$ErrorActionPreference = 'Stop'

$Root = $PSScriptRoot
if (-not $Root) { $Root = Split-Path -Parent $MyInvocation.MyCommand.Path }
Set-Location $Root

# 控制台统一 UTF-8，避免中文乱码
try {
    [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding($false)
    $OutputEncoding = [Console]::OutputEncoding
} catch { }

# ---------------------------------------------------------------------------
# 参数解析（兼容 --port 8787 / --port=8787 / -p 8787）
# ---------------------------------------------------------------------------
$Mode = 'dev'
$Port = 8787
$UiPort = 5173
$PortExplicit = $false
$UiPortExplicit = $false
$Release = $false
$Fresh = $false
$Restart = $false
$NoOpen = $false
$NoInstall = $false

$argv = @($args)
for ($i = 0; $i -lt $argv.Count; $i++) {
    $a = [string]$argv[$i]
    if ($a -match '^(?i)(dev|build|backend|frontend|status|stop|help)$') {
        $Mode = $Matches[1].ToLower()
    }
    elseif ($a -match '^--?port=(?<v>\d+)$') {
        $Port = [int]$Matches['v']
        $PortExplicit = $true
    }
    elseif ($a -match '^--?p$|^--?port$') {
        if ($i + 1 -ge $argv.Count) { throw "参数 $a 缺少端口号" }
        $i++
        $Port = [int]$argv[$i]
        $PortExplicit = $true
    }
    elseif ($a -match '^--?ui-port=(?<v>\d+)$') {
        $UiPort = [int]$Matches['v']
        $UiPortExplicit = $true
    }
    elseif ($a -match '^--?ui-port$') {
        if ($i + 1 -ge $argv.Count) { throw "参数 $a 缺少端口号" }
        $i++
        $UiPort = [int]$argv[$i]
        $UiPortExplicit = $true
    }
    elseif ($a -match '^--release$') { $Release = $true }
    elseif ($a -match '^--fresh$') { $Fresh = $true }
    elseif ($a -match '^--restart$|^--force$') { $Restart = $true }
    elseif ($a -match '^--no-open$') { $NoOpen = $true }
    elseif ($a -match '^--no-install$') { $NoInstall = $true }
    elseif ($a -match '^--help$|^-h$|^-\?$') { $Mode = 'help' }
    else { Write-Host "  [!!] 未知参数：$a（已忽略）" -ForegroundColor Yellow }
}

if ($Port -lt 1 -or $Port -gt 65535) { throw "后端端口不合法：$Port" }
if ($UiPort -lt 1 -or $UiPort -gt 65535) { throw "前端端口不合法：$UiPort" }
if ($Port -eq $UiPort) { throw "后端端口与前端端口不能相同" }

$BackendUrl = "http://127.0.0.1:$Port"
$UiUrl = "http://127.0.0.1:$UiPort"
$RunDir = Join-Path $Root 'run\dev'
$BackendPidFile = Join-Path $RunDir 'backend.pid'
$FrontendPidFile = Join-Path $RunDir 'frontend.pid'
$BackendPortFile = Join-Path $RunDir 'backend.port'
$FrontendPortFile = Join-Path $RunDir 'frontend.port'
$FrontendDir = Join-Path $Root 'frontend'
$DistDir = Join-Path $FrontendDir 'dist'

$script:DevPasswordUsed = $false
$script:FreshUsed = $Fresh

# ---------------------------------------------------------------------------
# 输出辅助
# ---------------------------------------------------------------------------
function Write-Banner {
    Write-Host ''
    Write-Host '  ============================================================' -ForegroundColor DarkCyan
    Write-Host '   FrpPanel Dev Launcher' -ForegroundColor Cyan
    Write-Host '   NFrp / 官方 frp  ·  Rust + Vue 3' -ForegroundColor DarkGray
    Write-Host '  ============================================================' -ForegroundColor DarkCyan
}

function Write-Step([string]$Text) {
    Write-Host "  [..] $Text" -ForegroundColor Cyan
}

function Write-Ok([string]$Text) {
    Write-Host "  [OK] $Text" -ForegroundColor Green
}

function Write-Warn([string]$Text) {
    Write-Host "  [!!] $Text" -ForegroundColor Yellow
}

function Write-Err([string]$Text) {
    Write-Host "  [XX] $Text" -ForegroundColor Red
}

# ---------------------------------------------------------------------------
# 基础探测
# ---------------------------------------------------------------------------
function Require-Command([string]$Name, [string]$Hint) {
    $cmd = Get-Command $Name -ErrorAction SilentlyContinue
    if (-not $cmd) { throw "未找到 $Name。$Hint" }
    return $cmd
}

function Test-PortInUse([int]$TargetPort) {
    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $iar = $client.BeginConnect('127.0.0.1', $TargetPort, $null, $null)
        if (-not $iar.AsyncWaitHandle.WaitOne(300)) { return $false }
        $client.EndConnect($iar)
        return $true
    } catch {
        return $false
    } finally {
        $client.Close()
    }
}

function Get-PortOwnerPid([int]$TargetPort) {
    try {
        $conn = Get-NetTCPConnection -LocalPort $TargetPort -State Listen -ErrorAction Stop |
            Select-Object -First 1
        if ($conn) { return [int]$conn.OwningProcess }
    } catch { }

    $lines = & netstat -ano -p TCP 2>$null
    foreach ($line in $lines) {
        if ($line -match ("^\s*TCP\s+\S+:" + $TargetPort + "\s+\S+\s+LISTENING\s+(\d+)")) {
            return [int]$Matches[1]
        }
    }
    return 0
}

function Get-ProcessCommandLine([int]$TargetPid) {
    try {
        $p = Get-CimInstance Win32_Process -Filter ("ProcessId=" + $TargetPid) -ErrorAction Stop
        return [string]$p.CommandLine
    } catch {
        return ''
    }
}

function Format-ProcessInfo([int]$TargetPid) {
    if ($TargetPid -le 0) { return '未知进程' }
    $p = Get-Process -Id $TargetPid -ErrorAction SilentlyContinue
    if (-not $p) { return "PID $TargetPid（已退出）" }
    return "PID $TargetPid · $($p.ProcessName)"
}

function Read-NumberFile([string]$File) {
    if (-not (Test-Path $File)) { return 0 }
    $raw = ([System.IO.File]::ReadAllText($File)).Trim()
    $n = 0
    if ([int]::TryParse($raw, [ref]$n)) { return $n }
    return 0
}

function Save-NumberFile([string]$File, [int]$Value) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $File) | Out-Null
    [System.IO.File]::WriteAllText($File, [string]$Value)
}

function Test-PanelPort([int]$TargetPort) {
    if (-not (Test-PortInUse $TargetPort)) { return $false }
    $owner = Get-PortOwnerPid $TargetPort
    if ($owner -le 0) { return $false }
    $proc = Get-Process -Id $owner -ErrorAction SilentlyContinue
    if ($proc -and $proc.ProcessName -eq 'frp-panel') { return $true }
    return [bool]((Get-ProcessCommandLine $owner) -match 'frp-panel')
}

function Test-VitePort([int]$TargetPort) {
    if (-not (Test-PortInUse $TargetPort)) { return $false }
    $owner = Get-PortOwnerPid $TargetPort
    if ($owner -le 0) { return $false }
    return [bool]((Get-ProcessCommandLine $owner) -match 'vite|npm run dev')
}

function Resolve-BackendPort {
    # 复用上一次 dev.bat 记录、且仍在服务的面板端口
    if (-not $PortExplicit) {
        $saved = Read-NumberFile $BackendPortFile
        if ($saved -gt 0 -and (Test-PanelPort $saved)) {
            $script:Port = $saved
            $script:BackendUrl = "http://127.0.0.1:$saved"
            Write-Ok "检测到已运行的面板，沿用端口 $saved"
            return
        }
    }

    if (-not (Test-PortInUse $Port)) { return }
    if (Test-PanelPort $Port) { return } # 已有面板，Start-Backend 会复用

    $owner = Get-PortOwnerPid $Port
    if ($PortExplicit) {
        throw "端口 $Port 被占用：$(Format-ProcessInfo $owner)。请换端口（--port 8899）或先释放该端口。"
    }

    foreach ($candidate in @(8899, 8788, 8790, 18080, 18787)) {
        if ($candidate -eq $Port) { continue }
        if (-not (Test-PortInUse $candidate)) {
            Write-Warn "默认端口 $Port 被 $(Format-ProcessInfo $owner) 占用，自动改用 $candidate"
            $script:Port = $candidate
            $script:BackendUrl = "http://127.0.0.1:$candidate"
            return
        }
    }
    throw "默认端口 $Port 被占用，且备用端口均不可用。请用 --port 指定空闲端口。"
}

function Resolve-UiPort {
    if (-not $UiPortExplicit) {
        $saved = Read-NumberFile $FrontendPortFile
        if ($saved -gt 0 -and (Test-VitePort $saved)) {
            $script:UiPort = $saved
            $script:UiUrl = "http://127.0.0.1:$saved"
            Write-Ok "检测到已运行的 Vite，沿用端口 $saved"
            return
        }
    }

    if (-not (Test-PortInUse $UiPort)) { return }
    if (Test-VitePort $UiPort) { return }

    $owner = Get-PortOwnerPid $UiPort
    if ($UiPortExplicit) {
        throw "前端端口 $UiPort 被占用：$(Format-ProcessInfo $owner)。请换端口（--ui-port 5174）或先释放。"
    }

    foreach ($candidate in @(5174, 5175, 4173, 5180)) {
        if ($candidate -eq $UiPort) { continue }
        if (-not (Test-PortInUse $candidate)) {
            Write-Warn "默认前端端口 $UiPort 被 $(Format-ProcessInfo $owner) 占用，自动改用 $candidate"
            $script:UiPort = $candidate
            $script:UiUrl = "http://127.0.0.1:$candidate"
            return
        }
    }
    throw "前端端口 $UiPort 被占用且无备用端口，请用 --ui-port 指定。"
}

function Resolve-ProxyTarget {
    # frontend 单启模式：找出正在运行的后端作为 Vite 代理目标
    if (Test-PanelPort $Port) { return }
    if (-not $PortExplicit) {
        $saved = Read-NumberFile $BackendPortFile
        if ($saved -gt 0 -and (Test-PanelPort $saved)) {
            $script:Port = $saved
            $script:BackendUrl = "http://127.0.0.1:$saved"
            Write-Ok "Vite 代理目标：$saved"
            return
        }
        foreach ($candidate in @(8899, 8788, 8790, 18080, 18787)) {
            if (Test-PanelPort $candidate) {
                $script:Port = $candidate
                $script:BackendUrl = "http://127.0.0.1:$candidate"
                Write-Ok "检测到正在运行的后端，Vite 代理目标：$candidate"
                return
            }
        }
    }
}


function Wait-Endpoint([string]$Url, [int]$Seconds, [string]$Label, [int]$ProcessId) {
    Write-Host "  [..] 等待 $Label 就绪" -ForegroundColor Cyan -NoNewline
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        if ($ProcessId -gt 0 -and -not (Get-Process -Id $ProcessId -ErrorAction SilentlyContinue)) {
            Write-Host ''
            Write-Warn "$Label 进程已退出（PID $ProcessId）"
            return $false
        }
        try {
            $resp = Invoke-WebRequest -Uri $Url -UseBasicParsing -TimeoutSec 2 -ErrorAction Stop
            if ($resp.StatusCode -ge 200 -and $resp.StatusCode -lt 500) {
                Write-Host '  就绪' -ForegroundColor Green
                return $true
            }
        } catch { }
        Start-Sleep -Milliseconds 700
        Write-Host '.' -NoNewline
    }
    Write-Host ''
    Write-Warn "$Label 在 ${Seconds}s 内未就绪"
    return $false
}

# ---------------------------------------------------------------------------
# 前置检查
# ---------------------------------------------------------------------------
function Test-Prerequisites {
    param([switch]$NeedNode)

    Require-Command 'cargo' '请安装 Rust 工具链：https://rustup.rs' | Out-Null
    Write-Ok "Rust $(& rustc --version)"

    if ($NeedNode) {
        Require-Command 'node' '请安装 Node.js 20+：https://nodejs.org' | Out-Null
        Require-Command 'npm' 'npm 应随 Node.js 一起安装' | Out-Null

        $nodeVersion = (& node --version) -replace '^v', ''
        $nodeMajor = [int]($nodeVersion.Split('.')[0])
        if ($nodeMajor -lt 20) {
            Write-Warn "当前 Node.js 为 v$nodeVersion，项目要求 >= 20（可能仍可运行）"
        } else {
            Write-Ok "Node.js v$nodeVersion · npm $(& npm --version)"
        }
    }

    # 复用 NFrp 的 nfrp-common（path 依赖）必须存在，否则编译会失败
    $cargoToml = Join-Path $Root 'Cargo.toml'
    if (Test-Path $cargoToml) {
        $tomlText = [System.IO.File]::ReadAllText($cargoToml)
        if ($tomlText -match 'nfrp-common\s*=\s*\{\s*path\s*=\s*"([^"]+)"') {
            $depPath = $Matches[1] -replace '/', '\'
            if (-not [System.IO.Path]::IsPathRooted($depPath)) {
                $depPath = Join-Path $Root $depPath
            }
            if (Test-Path $depPath) {
                Write-Ok "nfrp-common：$depPath"
            } else {
                throw @"
nfrp-common 路径不存在：$depPath

这是 Cargo.toml 里复用 NFrp 配置解析的 path 依赖。
请把 Cargo.toml 中的 nfrp-common path 改成你本机 NFrp 源码位置，例如：
  nfrp-common = { path = "E:/项目/RustFrp/NFrp/common" }
"@
            }
        }
    }
}

function Ensure-FrontendDeps {
    $viteMarker = Join-Path $FrontendDir 'node_modules\vite\package.json'
    if (Test-Path $viteMarker) { return }

    if ($NoInstall) {
        throw "frontend/node_modules 不完整，且指定了 --no-install。请先执行：cd frontend && npm install"
    }
    Write-Step '首次运行：安装前端依赖（npm install）'
    Push-Location $FrontendDir
    try {
        & npm install --no-audit --no-fund
        if ($LASTEXITCODE -ne 0) { throw "npm install 失败（exit $LASTEXITCODE）" }
    } finally {
        Pop-Location
    }
    Write-Ok '前端依赖安装完成'
}

# ---------------------------------------------------------------------------
# PID 与进程管理
# ---------------------------------------------------------------------------
function Read-PidFile([string]$File) {
    if (-not (Test-Path $File)) { return 0 }
    $raw = ([System.IO.File]::ReadAllText($File)).Trim()
    $n = 0
    if ([int]::TryParse($raw, [ref]$n)) { return $n }
    return 0
}

function Stop-ProcessTree([int]$TargetPid, [string]$Label, [string]$Expected) {
    if ($TargetPid -le 0) { return $false }
    $proc = Get-Process -Id $TargetPid -ErrorAction SilentlyContinue
    if (-not $proc) { return $false }

    $cmdLine = Get-ProcessCommandLine $TargetPid
    $looksOurs = $false
    if ($Expected -and $cmdLine -and $cmdLine -match $Expected) { $looksOurs = $true }
    if ($cmdLine -and $cmdLine -match [regex]::Escape($Root)) { $looksOurs = $true }
    if (-not $looksOurs) {
        Write-Warn "PID $TargetPid 不是本项目的进程（$($proc.ProcessName)），已跳过"
        return $false
    }

    Write-Step "停止 $Label（PID $TargetPid · $($proc.ProcessName)）"
    & taskkill /PID $TargetPid /T /F 2>&1 | Out-Null
    Start-Sleep -Milliseconds 400
    return $true
}

function Stop-ByPidFile([string]$File, [string]$Label, [string]$Expected) {
    $target = Read-PidFile $File
    if ($target -gt 0) {
        Stop-ProcessTree $target $Label $Expected | Out-Null
    }
    Remove-Item $File -Force -ErrorAction SilentlyContinue
}

function Stop-Orphans {
    # 清理 PID 文件丢失时仍残留的、明确属于本项目的进程
    try {
        $candidates = Get-CimInstance Win32_Process -ErrorAction Stop | Where-Object {
            $_.CommandLine -and (
                ($_.Name -match '^frp-panel(\.exe)?$' -and $_.CommandLine -match [regex]::Escape($Root)) -or
                ($_.Name -match '^(cargo|node|cmd)(\.exe)?$' -and $_.CommandLine -match [regex]::Escape($Root) -and
                    $_.CommandLine -match 'frp-panel|vite|npm run dev')
            )
        }
        foreach ($c in $candidates) {
            if ($c.ProcessId -eq $PID) { continue }
            Write-Step "清理残留进程：PID $($c.ProcessId) · $($c.Name)"
            & taskkill /PID $c.ProcessId /T /F 2>&1 | Out-Null
        }
    } catch {
        Write-Warn "扫描残留进程失败：$($_.Exception.Message)"
    }
}

# ---------------------------------------------------------------------------
# 构建 / 启动
# ---------------------------------------------------------------------------
function Invoke-BackendBuild {
    $cargoArgs = @('build')
    if ($Release) { $cargoArgs += '--release' }
    $cargoArgs += @('-p', 'frp-panel')

    Write-Step ("编译后端：cargo " + ($cargoArgs -join ' '))
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & cargo @cargoArgs
    if ($LASTEXITCODE -ne 0) { throw "后端编译失败（cargo exit $LASTEXITCODE）" }
    $sw.Stop()
    Write-Ok ("后端编译完成（{0:N1}s）" -f $sw.Elapsed.TotalSeconds)
}

function Invoke-FrontendBuild {
    Ensure-FrontendDeps
    Write-Step '构建前端：npm run build'
    Push-Location $FrontendDir
    try {
        & npm run build
        if ($LASTEXITCODE -ne 0) { throw "前端构建失败（exit $LASTEXITCODE）" }
    } finally {
        Pop-Location
    }
    if (-not (Test-Path (Join-Path $DistDir 'index.html'))) {
        throw "前端构建产物不完整：$DistDir\index.html 不存在"
    }
    Write-Ok "前端构建完成：$DistDir"
}

function Start-DevWindow([string]$Title, [string]$Command, [string]$WorkDir, [string]$PidFile) {
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $PidFile) | Out-Null
    $full = 'title ' + $Title + ' && ' + $Command
    $proc = Start-Process -FilePath $env:ComSpec `
        -ArgumentList @('/d', '/k', ('"' + $full + '"')) `
        -WorkingDirectory $WorkDir -PassThru
    [System.IO.File]::WriteAllText($PidFile, [string]$proc.Id)
    return $proc
}

function Start-Backend([switch]$ServeStatic) {
    if (Test-PortInUse $Port) {
        $owner = Get-PortOwnerPid $Port
        $cmdLine = Get-ProcessCommandLine $owner
        $isPanel = $cmdLine -match 'frp-panel' -or (Get-Process -Id $owner -ErrorAction SilentlyContinue).ProcessName -eq 'frp-panel'

        if ($Restart) {
            Stop-ByPidFile $BackendPidFile '旧后端' 'frp-panel|cargo run'
            if ($isPanel -and $owner -gt 0) {
                Write-Step "停止占用 $Port 的面板进程（$(Format-ProcessInfo $owner)）"
                & taskkill /PID $owner /T /F 2>&1 | Out-Null
                Start-Sleep -Milliseconds 600
            }
        } elseif ($isPanel) {
            Write-Ok "后端已在运行：$BackendUrl（$(Format-ProcessInfo $owner)）"
            Save-NumberFile $BackendPortFile $Port
            return $true
        } else {
            throw "端口 $Port 被其他程序占用：$(Format-ProcessInfo $owner)。请换端口（--port 8899）或加 --restart"
        }
    }

    if (Test-PortInUse $Port) {
        $owner = Get-PortOwnerPid $Port
        throw "端口 $Port 仍被占用：$(Format-ProcessInfo $owner)。请手动处理或换端口。"
    }

    Invoke-BackendBuild

    # 环境变量：子进程继承
    if (-not $env:RUST_LOG) {
        $env:RUST_LOG = 'info,frp_panel=debug,tower_http=warn,russh=warn'
    }
    $env:FRP_PANEL_STATIC_DIR = ''

    if ($Fresh) {
        $env:FRP_PANEL_DATA_DIR = 'data/dev'
        if (-not $env:FRP_PANEL_ADMIN_PASSWORD) {
            $env:FRP_PANEL_ADMIN_PASSWORD = 'paneladmin123'
            $script:DevPasswordUsed = $true
        }
        Write-Step '使用隔离数据目录：data/dev（不影响 data/panel.db）'
    } else {
        if (Test-Path (Join-Path $Root 'data\panel.db')) {
            # 已有数据库：不要覆盖用户口令
        } elseif (-not $env:FRP_PANEL_ADMIN_PASSWORD) {
            $env:FRP_PANEL_ADMIN_PASSWORD = 'paneladmin123'
            $script:DevPasswordUsed = $true
            Write-Warn '首次初始化：使用开发口令 paneladmin123（仅回环监听）'
        }
    }

    if ($ServeStatic) {
        $env:FRP_PANEL_STATIC_DIR = $DistDir
        Write-Step "后端托管静态资源：$DistDir"
    }

    $cargoArgs = @('run')
    if ($Release) { $cargoArgs += '--release' }
    $cargoArgs += @('-p', 'frp-panel', '--', '--bind', "127.0.0.1:$Port")
    $cmd = 'cargo ' + ($cargoArgs -join ' ')

    Write-Step "启动后端窗口：$cmd"
    $proc = Start-DevWindow "FrpPanel Backend :$Port" $cmd $Root $BackendPidFile

    if (-not (Wait-Endpoint "$BackendUrl/api/health" 90 '后端' $proc.Id)) {
        throw "后端未能启动。请查看「FrpPanel Backend :$Port」窗口中的编译/运行日志。"
    }
    Save-NumberFile $BackendPortFile $Port
    return $true
}

function Start-Frontend {
    if (Test-PortInUse $UiPort) {
        $owner = Get-PortOwnerPid $UiPort
        $cmdLine = Get-ProcessCommandLine $owner
        $isVite = $cmdLine -match 'vite' -or $cmdLine -match 'npm run dev'

        if ($Restart) {
            Stop-ByPidFile $FrontendPidFile '旧前端' 'vite|npm run dev'
            if ($isVite -and $owner -gt 0) {
                Write-Step "停止占用 $UiPort 的 Vite 进程（$(Format-ProcessInfo $owner)）"
                & taskkill /PID $owner /T /F 2>&1 | Out-Null
                Start-Sleep -Milliseconds 500
            }
        } elseif ($isVite) {
            Write-Ok "前端已在运行：$UiUrl（$(Format-ProcessInfo $owner)）"
            Save-NumberFile $FrontendPortFile $UiPort
            return $true
        } else {
            throw "端口 $UiPort 被其他程序占用：$(Format-ProcessInfo $owner)。请换端口（--ui-port 5174）或加 --restart"
        }
    }

    Ensure-FrontendDeps

    # Vite 的 /api 代理目标
    $env:FRP_PANEL_API = $BackendUrl
    $cmd = "npm run dev -- --host 127.0.0.1 --port $UiPort --strictPort"

    Write-Step "启动前端窗口：$cmd（API -> $BackendUrl）"
    $proc = Start-DevWindow "FrpPanel Frontend :$UiPort" $cmd $FrontendDir $FrontendPidFile

    if (-not (Wait-Endpoint $UiUrl 60 '前端' $proc.Id)) {
        throw "Vite 未能启动。请查看「FrpPanel Frontend :$UiPort」窗口中的日志。"
    }
    Save-NumberFile $FrontendPortFile $UiPort
    return $true
}

# ---------------------------------------------------------------------------
# 信息输出
# ---------------------------------------------------------------------------
function Get-AdminHint {
    $dataDir = if ($script:FreshUsed) { Join-Path $Root 'data\dev' } else { Join-Path $Root 'data' }
    $file = Join-Path $dataDir 'admin.initial'
    if (Test-Path $file) {
        try {
            $text = [System.IO.File]::ReadAllText($file, [System.Text.Encoding]::UTF8)
            if ($text -match '密码[:：]\s*(\S+)') {
                return "admin / $($Matches[1])（来自 admin.initial）"
            }
        } catch { }
    }
    if ($script:DevPasswordUsed) {
        return 'admin / paneladmin123（首次初始化）'
    }
    return 'admin / 你之前设置的口令（已有数据库，不会重置）'
}

function Show-Summary([string]$PanelUrl, [switch]$WithFrontend, [switch]$ApiOnly) {
    Write-Host ''
    Write-Host '  ------------------------------------------------------------' -ForegroundColor DarkCyan

    if ($WithFrontend) {
        Write-Host '   面板地址   ' -NoNewline -ForegroundColor Gray
        Write-Host $PanelUrl -ForegroundColor Cyan
        Write-Host '   Vite HMR   ' -NoNewline -ForegroundColor Gray
        Write-Host "已启用（修改 frontend/src 自动刷新）" -ForegroundColor Green
    } elseif ($ApiOnly) {
        Write-Host '   后端 API   ' -NoNewline -ForegroundColor Gray
        Write-Host $PanelUrl -ForegroundColor Cyan
        Write-Host '   前端       ' -NoNewline -ForegroundColor Gray
        Write-Host '未启动（当前仅后端；可另开 dev.bat frontend）' -ForegroundColor DarkGray
    } else {
        Write-Host '   面板地址   ' -NoNewline -ForegroundColor Gray
        Write-Host $PanelUrl -ForegroundColor Cyan
    }

    Write-Host '   后端 API   ' -NoNewline -ForegroundColor Gray
    Write-Host "$BackendUrl/api/health" -ForegroundColor DarkGray
    Write-Host '   登录账号   ' -NoNewline -ForegroundColor Gray
    Write-Host (Get-AdminHint) -ForegroundColor White
    Write-Host '   数据目录   ' -NoNewline -ForegroundColor Gray
    if ($script:FreshUsed) {
        Write-Host (Join-Path $Root 'data\dev') -ForegroundColor DarkGray
    } else {
        Write-Host (Join-Path $Root 'data') -ForegroundColor DarkGray
    }
    Write-Host '   停止服务   ' -NoNewline -ForegroundColor Gray
    Write-Host 'dev.bat stop' -ForegroundColor White
    Write-Host '   查看状态   ' -NoNewline -ForegroundColor Gray
    Write-Host 'dev.bat status' -ForegroundColor White
    Write-Host '   运行日志   ' -NoNewline -ForegroundColor Gray
    Write-Host '见对应的 Backend / Frontend 控制台窗口' -ForegroundColor DarkGray
    Write-Host '  ------------------------------------------------------------' -ForegroundColor DarkCyan
    Write-Host ''

    if (-not $NoOpen) {
        Write-Step "打开浏览器：$PanelUrl"
        try { Start-Process $PanelUrl | Out-Null } catch { Write-Warn "无法自动打开浏览器：$($_.Exception.Message)" }
    }
}

function Show-Help {
    Write-Banner
    Write-Host @'
  用法：
      dev.bat [模式] [选项]

  模式：
      dev        后端 + 前端 Vite（默认，支持热更新）
      build      构建前端后由后端单进程托管（接近生产）
      backend    只启动后端
      frontend   只启动前端（后端需已运行）
      status     查看端口、健康状态和 PID
      stop       停止由 dev.bat 启动的进程
      help       显示本帮助

  选项：
      --port N        后端端口（默认 8787）
      --ui-port N     前端端口（默认 5173）
      --release       后端使用 release 构建
      --fresh         使用隔离数据目录 data/dev
                      首次管理员：admin / paneladmin123
      --restart       启动前停止本启动器记录的旧进程
      --no-open       不自动打开浏览器
      --no-install    前端依赖缺失时不自动 npm install

  示例：
      dev.bat                         本地开发（推荐）
      dev.bat --port 8899             换后端端口
      dev.bat --fresh                 干净数据目录，避免影响现有实例
      dev.bat build                   构建前端并单进程运行
      dev.bat stop                    停止服务
      dev.bat status                  检查是否在运行

  说明：
      - 后端默认监听 127.0.0.1，仅本机可访问。
      - 开发模式前端 http://127.0.0.1:5173，/api 由 Vite 代理到后端。
      - 首次启动会在 data/ 生成 panel.db 与密钥文件；请勿提交到仓库。
'@ -ForegroundColor Gray
    Write-Host ''
}

# ---------------------------------------------------------------------------
# 主流程
# ---------------------------------------------------------------------------
try {
    if ($Mode -eq 'help') {
        Show-Help
        exit 0
    }

    New-Item -ItemType Directory -Force -Path $RunDir | Out-Null
    Write-Banner

    if ($Mode -eq 'stop') {
        if (-not $PortExplicit) {
            $saved = Read-NumberFile $BackendPortFile
            if ($saved -gt 0) { $Port = $saved; $BackendUrl = "http://127.0.0.1:$saved" }
        }
        if (-not $UiPortExplicit) {
            $savedUi = Read-NumberFile $FrontendPortFile
            if ($savedUi -gt 0) { $UiPort = $savedUi; $UiUrl = "http://127.0.0.1:$savedUi" }
        }

        Stop-ByPidFile $BackendPidFile '后端' 'frp-panel|cargo run'
        Stop-ByPidFile $FrontendPidFile '前端' 'vite|npm run dev'
        Stop-Orphans
        Remove-Item $BackendPortFile -Force -ErrorAction SilentlyContinue
        Remove-Item $FrontendPortFile -Force -ErrorAction SilentlyContinue

        if (Test-PortInUse $Port) {
            Write-Warn "端口 $Port 仍有监听：$(Format-ProcessInfo (Get-PortOwnerPid $Port))"
        } else {
            Write-Ok "端口 $Port 已释放"
        }
        if (Test-PortInUse $UiPort) {
            Write-Warn "端口 $UiPort 仍有监听：$(Format-ProcessInfo (Get-PortOwnerPid $UiPort))"
        } else {
            Write-Ok "端口 $UiPort 已释放"
        }
        Write-Host ''
        exit 0
    }

    if ($Mode -eq 'status') {
        if (-not $PortExplicit) {
            $saved = Read-NumberFile $BackendPortFile
            if ($saved -gt 0) { $Port = $saved; $BackendUrl = "http://127.0.0.1:$saved" }
        }
        if (-not $UiPortExplicit) {
            $savedUi = Read-NumberFile $FrontendPortFile
            if ($savedUi -gt 0) { $UiPort = $savedUi; $UiUrl = "http://127.0.0.1:$savedUi" }
        }

        $healthOk = $false
        try {
            $h = Invoke-RestMethod -Uri "$BackendUrl/api/health" -TimeoutSec 3
            Write-Ok "后端在线：$($h.status) · FrpPanel $($h.version) · $BackendUrl"
            $healthOk = $true
        } catch {
            Write-Warn "后端未响应：$BackendUrl"
        }

        $bp = Read-PidFile $BackendPidFile
        if ($bp -gt 0) { Write-Host "       后端 PID 文件：$bp（$(Format-ProcessInfo $bp)）" -ForegroundColor DarkGray }
        $fp = Read-PidFile $FrontendPidFile
        if ($fp -gt 0) { Write-Host "       前端 PID 文件：$fp（$(Format-ProcessInfo $fp)）" -ForegroundColor DarkGray }

        if (Test-PortInUse $UiPort) {
            Write-Ok "前端监听中：$UiUrl（$(Format-ProcessInfo (Get-PortOwnerPid $UiPort))）"
        } else {
            Write-Warn "前端未监听：$UiUrl"
        }
        if (Test-PortInUse $Port) {
            Write-Host "       后端端口 $Port 监听进程：$(Format-ProcessInfo (Get-PortOwnerPid $Port))" -ForegroundColor DarkGray
        }

        Write-Host ''
        exit 0
    }

    # 以下模式需要前端依赖
    $needNode = $Mode -in @('dev', 'build', 'frontend')
    Write-Step '检查运行环境'
    Test-Prerequisites -NeedNode:$needNode
    if ($needNode) { Ensure-FrontendDeps }

    if ($Mode -in @('dev', 'build', 'backend')) { Resolve-BackendPort }
    if ($Mode -eq 'frontend') { Resolve-ProxyTarget }
    if ($Mode -in @('dev', 'frontend')) { Resolve-UiPort }

    switch ($Mode) {
        'build' {
            Invoke-FrontendBuild
            Start-Backend -ServeStatic | Out-Null
            Show-Summary $BackendUrl
        }
        'backend' {
            Start-Backend | Out-Null
            Show-Summary "$BackendUrl/api/health" -ApiOnly
        }
        'frontend' {
            if (-not (Test-PanelPort $Port)) {
                Write-Warn "后端尚未监听 $BackendUrl；前端能打开，但接口会失败。可另开窗口运行 dev.bat backend"
            }
            Start-Frontend | Out-Null
            Show-Summary $UiUrl -WithFrontend
        }
        default {
            Start-Backend | Out-Null
            Start-Frontend | Out-Null
            Show-Summary $UiUrl -WithFrontend
        }
    }

    exit 0
} catch {
    Write-Host ''
    Write-Err $_.Exception.Message
    if ($_.ScriptStackTrace) {
        Write-Host "       $($_.ScriptStackTrace)" -ForegroundColor DarkGray
    }
    Write-Host ''
    Write-Host '  提示：加上 --fresh 可使用隔离数据目录；--restart 可清理旧进程；' -ForegroundColor DarkGray
    Write-Host '        dev.bat help 查看全部用法。' -ForegroundColor DarkGray
    exit 1
}
