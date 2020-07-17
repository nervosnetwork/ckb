# CKB on Windows

## Build CKB on Windows 10

**All commands should be ran as PowerShell commands.**

### Setup the Build Environment

#### Install Visual Studio 2019

Install [Visual Studio 2019](https://visualstudio.microsoft.com/downloads/)
with the workload: "Desktop development with C++".

**(minimum required)** Or you can just select two individual components:
"MSVC v142 - VS 2019 C++ x64/x86 build tools (vXX.XX)" and "Windows 10 SDK (10.0.X.0)".

#### Install Tools with [Scoop]

- Install [Scoop].

- Install `git`, `llvm`, `yasm` and `rustup` via [Scoop].

  ```posh
  scoop install git llvm yasm rustup
  ```

- Install dependencies.

  - Add ["extras" bucket](https://github.com/lukesampson/scoop-extras) for [Scoop].

    ```posh
    scoop bucket add extras
    ```

  - `yasm` requires Microsoft Visual C++ 2010 runtime Libraries.

    ```posh
    scoop install vcredist2010
    ```

- Configure `Rust`.

  ```posh
  rustup set default-host x86_64-pc-windows-msvc
  ```

### Build CKB on Windows 10

- Checkout the source code of [CKB].

  ```posh
  git clone https://github.com/nervosnetwork/ckb
  cd ckb
  ```

- Build [CKB].

  ```posh
  devtools/windows/make prod
  ```

[CKB]: https://github.com/nervosnetwork/ckb
[Scoop]: https://scoop.sh/
