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
      Remove-Item -Path "env:$($item.Key)"
    } else {
      echo "set $($item.Key)=$($item.Value)"
      Set-Item -Path "env:$($item.Key)" -Value $item.Value
    }
  }
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
  mkdir -Force -ErrorAction SilentlyContinue target
  New-Item -ItemType Junction -Path test/target -Value "$(pwd)/target"

  cargo build --features deadlock_detection

  pushd test
  Set-Env RUST_BACKTRACE 1
  Set-Env RUST_LOG $env:INTEGRATION_RUST_LOG
  iex "cargo run -- --bin target/debug/ckb $($env:CKB_TEST_ARGS)"
  popd
}

try {
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
