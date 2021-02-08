$ErrorActionPreference = "Stop"
$RememberEnv = @{}
$Verbose = @()
if ($env:CI -ne $null) {
  $Verbose = @("--verbose")
}

function Set-Env {
  param(
    [string] $Key,
    [string] $Value
  )

  if (-not $RememberEnv.ContainsKey($Key)) {
    $RememberEnv[$Key] = (Get-Item -Path "env:$Key" -ErrorAction SilentlyContinue).Value
  }
  echo "set $Key=$Value"
  Set-Item -Path "env:$Key" -Value $Value
}

function Restore-Env {
  foreach ($item in $RememberEnv.GetEnumerator()) {
    if ($item.Value -eq $null) {
      echo "unset $($item.Key)"
      Remove-Item -Path "env:$($item.Key)" -ErrorAction SilentlyContinue
    } else {
      echo "set $($item.Key)=$($item.Value)"
      Set-Item -Path "env:$($item.Key)" -Value $item.Value
    }
  }
}

function Unset-Env {
  param(
    [string] $Key
  )

  echo "unset $Key"
  Remove-Item -Path "env:\$Key" -ErrorAction SilentlyContinue
}

function Enable-DebugSymbols {
  $content = Get-Content Cargo.toml | % {
    $_
    if ($_ -eq '[profile.release]') {
      "debug = true"
    }
  }
  ($content -join "`n") + "`n" | Set-Content -NoNewline -Path Cargo.toml
}

function Disable-DebugSymbols {
  $content = Get-Content Cargo.toml | ? {
    $_ -ne "debug = true"
  }
  ($content -join "`n") + "`n" | Set-Content -NoNewline -Path Cargo.toml
}

function run-prod {
  Set-Env RUSTFLAGS "--cfg disable_faketime"
  cargo build @Verbose --release
}

function run-prod-with-debug {
  try {
    Enable-DebugSymbols
    run-prod
  } finally {
    Disable-DebugSymbols
  }
}

function run-test {
  cargo test @Verbose --all -- --nocapture
}

function run-integration {
  git submodule update --init
  cp -Fo Cargo.lock test/Cargo.lock
  rm -Re -Fo -ErrorAction SilentlyContinue test/target

  cargo build --features deadlock_detection
  New-Item -ItemType Junction -Path test/target -Value "$(pwd)/target"

  pushd test

  Set-Env RUST_BACKTRACE 1
  Set-Env RUST_LOG $env:INTEGRATION_RUST_LOG

  $test_id=$(Get-Date -UFormat "%Y%m%d-%H%M%S")
  if ($env:CKB_INTEGRATION_TEST_TMP -eq $null) {
    Set-Env CKB_INTEGRATION_TEST_TMP "target/ckb-test/$test_id"
  }
  if ($env:CKB_INTEGRATION_FAILURE_FILE -eq $null) {
    Set-Env CKB_INTEGRATION_FAILURE_FILE "$env:CKB_INTEGRATION_TEST_TMP/integration.failure"
  }
  New-Item -Path "$env:CKB_INTEGRATION_TEST_TMP" -Type Directory -ErrorAction SilentlyContinue

  $ckb_bin="target/debug/ckb"
  $logfile="$env:CKB_INTEGRATION_TEST_TMP/integration.log"
  $ckb_release=$(iex "$ckb_bin --version")

  iex "cargo run -- --bin $ckb_bin --log-file ${logfile} $($env:CKB_TEST_ARGS)"
  $errcode=$LASTEXITCODE

  if ($errcode -ne 0) {
    if ($env:LOGBAK_SERVER -ne $null) {
        $upload_id="azure-$test_id-$($env:BUILD_BUILDID ?? "0")-$($env:ImageOS ?? "unknown")"
        7z a -t7z "$upload_id.7z" "$env:CKB_INTEGRATION_TEST_TMP"
        echo y | pscp -sftp -P 22 -pw "${env:LOGBAK_PASSWORD}" "$upload_id.7z" "${env:LOGBAK_USER}@${env:LOGBAK_SERVER}:/ci/azure/"
    }
    Unset-Env LOGBAK_USER
    Unset-Env LOGBAK_PASSWORD
    Unset-Env LOGBAK_SERVER
    if ($env:SENTRY_DSN -ne $null) {
      foreach($line in Get-Content $env:CKB_INTEGRATION_FAILURE_FILE) {
        sentry-cli send-event -m "$line" -r "$ckb_release" --logfile "$logfile"
      }
    }
    exit $errcode
  }

  popd
}

function run-gen-rpc-doc {
  rm -ErrorAction SilentlyContinue -Force target/doc/ckb_rpc/module/trait.*.html
  cargo doc -p ckb-rpc -p ckb-types -p ckb-fixed-hash -p ckb-fixed-hash-core -p ckb-jsonrpc-types --no-deps
  python3 ./devtools/doc/rpc.py > rpc/README.md
}

try {
  if ($env:CKB_TEST_ARGS -eq $null) {
    Set-Env CKB_TEST_ARGS "-c 4"
  }
  if ($env:INTEGRATION_RUST_LOG -eq $null) {
    Set-Env INTEGRATION_RUST_LOG "info,ckb_test=debug,ckb_sync=debug,ckb_relay=debug,ckb_network=debug"
  }

  foreach ($arg in $args) {
    $parts = $arg -Split '=', 2
      if ($parts.Count -eq 2) {
        Set-Env $parts[0] $parts[1]
      } else {
        echo "Run $arg"
        & "run-$arg"
      }
  }
} finally {
  Restore-Env
}
