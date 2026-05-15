param(
    [Parameter(Position = 0)]
    [string]$Framework = "all",

    [Parameter(Position = 1)]
    [string]$BaseUrl = "",

    [int]$Vus = 20,
    [string]$Duration = "30s",
    [string]$OutDir = "",
    [string]$DockerContext = "default",
    [string]$K6Image = "grafana/k6:latest",
    [ValidateRange(1, 100)]
    [int]$Repeat = 1,
    [string[]]$Scripts = @("json-crud", "html-page", "validation-fail", "auth-protected")
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$ScriptDir = $PSScriptRoot
$RuntimeDir = Resolve-Path (Join-Path $ScriptDir "..")
$K6Dir = Resolve-Path (Join-Path $ScriptDir "k6")

function Get-Median {
    param([double[]]$Values)

    $sorted = @($Values | Sort-Object)
    if ($sorted.Count -eq 0) {
        return $null
    }

    $middle = [int][math]::Floor($sorted.Count / 2)
    if ($sorted.Count % 2 -eq 1) {
        return $sorted[$middle]
    }

    return ($sorted[$middle - 1] + $sorted[$middle]) / 2
}

if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $stamp = Get-Date -Format "yyyyMMdd_HHmmss"
    $OutDir = Join-Path $RuntimeDir "results\docker-k6_$stamp"
}

$OutDir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($OutDir)

$FrameworkUrls = [ordered]@{
    "autumn"      = "http://host.docker.internal:8001"
    "spring-boot" = "http://host.docker.internal:8002"
    "rails"       = "http://host.docker.internal:8003"
    "django"      = "http://host.docker.internal:8004"
    "phoenix"     = "http://host.docker.internal:8005"
    "loco"        = "http://host.docker.internal:8006"
}

if ($Framework -eq "all") {
    $Targets = @($FrameworkUrls.Keys)
} elseif ($FrameworkUrls.Contains($Framework)) {
    $Targets = @($Framework)
} else {
    if ([string]::IsNullOrWhiteSpace($BaseUrl)) {
        $known = $FrameworkUrls.Keys -join ", "
        throw "Unknown framework '$Framework'. Known frameworks: $known"
    }
    $Targets = @($Framework)
}

$HadFailure = $false

for ($run = 1; $run -le $Repeat; $run++) {
    $runName = "run-{0:D2}" -f $run
    $runRoot = if ($Repeat -eq 1) { $OutDir } else { Join-Path $OutDir $runName }

    foreach ($fw in $Targets) {
        $url = if ([string]::IsNullOrWhiteSpace($BaseUrl)) { $FrameworkUrls[$fw] } else { $BaseUrl }
        $fwOut = Join-Path $runRoot $fw
        New-Item -ItemType Directory -Force -Path $fwOut | Out-Null

        Write-Host ""
        Write-Host "========================================"
        Write-Host "Run:       $runName of $Repeat"
        Write-Host "Framework: $fw"
        Write-Host "Base URL:  $url"
        Write-Host "VUs:       $Vus"
        Write-Host "Duration:  $Duration"
        Write-Host "Output:    $fwOut"
        Write-Host "========================================"

        foreach ($script in $Scripts) {
            $scriptPath = Join-Path $K6Dir "$script.js"
            if (-not (Test-Path $scriptPath)) {
                throw "Missing k6 script: $scriptPath"
            }

            Write-Host ""
            Write-Host "--- Running $script ---"

            docker --context $DockerContext run --rm `
                -e BASE_URL=$url `
                -e BENCH_TOKEN=benchmark-token-abc123 `
                -e VUS=$Vus `
                -e DURATION=$Duration `
                -v "${K6Dir}:/scripts:ro" `
                -v "${fwOut}:/results" `
                $K6Image run --quiet `
                --summary-export "/results/$script-summary.json" `
                "/scripts/$script.js"

            if ($LASTEXITCODE -ne 0) {
                $HadFailure = $true
                Write-Host "k6 exit code $LASTEXITCODE for $fw/$script"
            }
        }
    }
}

$summaryFiles = @(Get-ChildItem -Path $OutDir -Recurse -Filter "*-summary.json" -ErrorAction SilentlyContinue)
if ($summaryFiles.Count -gt 0) {
    $rows = foreach ($file in $summaryFiles) {
        $json = Get-Content $file.FullName -Raw | ConvertFrom-Json
        $metrics = $json.metrics
        $relativeDir = $file.Directory.FullName.Substring($OutDir.Length).TrimStart("\", "/")
        $parts = $relativeDir -split "[\\/]"
        $runName = if ($Repeat -eq 1) { "run-01" } else { $parts[0] }
        $frameworkName = if ($Repeat -eq 1) { $parts[0] } else { $parts[1] }
        [pscustomobject]@{
            run             = $runName
            framework       = $frameworkName
            scenario        = $file.BaseName -replace "-summary$", ""
            req_s           = [math]::Round($metrics.http_reqs.rate, 1)
            p95_ms          = [math]::Round($metrics.http_req_duration.'p(95)', 1)
            avg_ms          = [math]::Round($metrics.http_req_duration.avg, 1)
            checks_pass_pct = [math]::Round($metrics.checks.value * 100, 2)
            check_fails     = $metrics.checks.fails
            http_failed_pct = [math]::Round($metrics.http_req_failed.value * 100, 2)
        }
    }

    $aggregatePath = Join-Path $OutDir "aggregate.csv"
    $rows | Sort-Object run, framework, scenario | Export-Csv -NoTypeInformation -Path $aggregatePath

    $summaryRows = foreach ($group in ($rows | Group-Object framework, scenario)) {
        $items = @($group.Group)
        $reqValues = [double[]]($items | ForEach-Object { [double]$_.req_s })
        $p95Values = [double[]]($items | ForEach-Object { [double]$_.p95_ms })
        $avgValues = [double[]]($items | ForEach-Object { [double]$_.avg_ms })
        $checkPassValues = [double[]]($items | ForEach-Object { [double]$_.checks_pass_pct })
        $httpFailedValues = [double[]]($items | ForEach-Object { [double]$_.http_failed_pct })

        [pscustomobject]@{
            framework                  = $items[0].framework
            scenario                   = $items[0].scenario
            runs                       = $items.Count
            median_req_s               = [math]::Round((Get-Median $reqValues), 1)
            median_p95_ms              = [math]::Round((Get-Median $p95Values), 1)
            median_avg_ms              = [math]::Round((Get-Median $avgValues), 1)
            median_checks_pass_pct     = [math]::Round((Get-Median $checkPassValues), 2)
            total_check_fails          = ($items | Measure-Object -Property check_fails -Sum).Sum
            median_http_failed_pct     = [math]::Round((Get-Median $httpFailedValues), 2)
            min_p95_ms                 = [math]::Round(($p95Values | Measure-Object -Minimum).Minimum, 1)
            max_p95_ms                 = [math]::Round(($p95Values | Measure-Object -Maximum).Maximum, 1)
        }
    }

    $summaryPath = Join-Path $OutDir "aggregate-summary.csv"
    $summaryRows | Sort-Object framework, scenario | Export-Csv -NoTypeInformation -Path $summaryPath

    Write-Host ""
    Write-Host "Aggregate:"
    $rows | Sort-Object run, framework, scenario | Format-Table -AutoSize
    Write-Host "Aggregate CSV: $aggregatePath"
    Write-Host ""
    Write-Host "Aggregate summary:"
    $summaryRows | Sort-Object framework, scenario | Format-Table -AutoSize
    Write-Host "Aggregate summary CSV: $summaryPath"
}

Write-Host ""
Write-Host "Results written to: $OutDir"

if ($HadFailure) {
    exit 1
}
