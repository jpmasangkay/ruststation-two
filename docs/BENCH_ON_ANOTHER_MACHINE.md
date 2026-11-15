# Benchmarking `ps2-autotest` on Another Windows Machine

This guide is for running the headless CPU/VU/DMAC/VIF/GS hardware benchmark (`ps2-autotest`) on another Windows PC **without installing Rust or any build tools**.

> **Important (Legal & Privacy):**
> - **No PS2 BIOS dump (`scph*.bin`) and no game discs/ISOs are needed.** Do **not** copy any BIOS or game files to the test machine or include them in your report.
> - **No third-party test binaries are bundled in our CI artifact.** You clone [`unknownbrackets/ps2autotests`](https://github.com/unknownbrackets/ps2autotests.git) directly on the test machine.
> - This measures the **current headless interpreter throughput** (Phase 1 + Phase 2a suites). JIT and GPU hardware rendering are not part of this benchmark yet.

---

## Prerequisites

- A Windows 10 or Windows 11 (64-bit x86-64) PC.
- **Git** installed so `git clone` works in PowerShell or Command Prompt (check with `git --version`; if missing, install [Git for Windows](https://git-scm.com/download/win)).
- **No Rust, Cargo, or Visual Studio build tools required.**

*(Troubleshooting: If Windows reports that `VCRUNTIME140.dll` was not found when launching `ps2-autotest.exe`, install Microsoft's official "Visual C++ Redistributable for Visual Studio (x64)".)*

---

## Step-by-Step Instructions

1. **Prepare power settings (especially on laptops):**
   - Plug the laptop into **AC wall power** (do not benchmark on battery unless specifically testing battery behavior).
   - Close heavy background programs (browsers, games, updaters).
   - Check and note your active Windows **Power mode** (**Settings -> System -> Power & battery -> Power mode**, e.g. *Best performance* or *Balanced*, plus any OEM laptop fan/performance mode if applicable).

2. **Download the Windows `ps2-autotest` binary artifact from GitHub Actions:**
   - Open this repository on GitHub in a web browser and click the **Actions** tab.
   - Open the latest green (passing) **CI** workflow run on `main`.
   - Scroll down to the **Artifacts** section at the bottom of the summary page.
   - Click **`ps2-autotest-windows-x86_64`** to download the zip archive.
     - *(Note: The artifact contains **only** `ps2-autotest.exe` — zero BIOS files, zero game files, and zero third-party test binaries.)*
   - Extract `ps2-autotest.exe` into a clean folder, for example `C:\ps2bench\`, so the file is at:
     `C:\ps2bench\ps2-autotest.exe`

3. **Clone `ps2autotests` into a folder:**
   Open **PowerShell** (or **Command Prompt**), change into your benchmark folder, and clone the upstream test suite:
   ```powershell
   cd C:\ps2bench
   git clone --depth 1 https://github.com/unknownbrackets/ps2autotests.git ps2autotests
   ```
   *(You do **not** need to clone the PS2 emulator repository itself.)*

4. **Run the benchmark:**
   In the same terminal inside `C:\ps2bench`, run:
   ```powershell
   .\ps2-autotest.exe --bench --tests-dir .\ps2autotests
   ```
   - Both the cloned repository root (`.\ps2autotests`) and its `tests` subfolder (`.\ps2autotests\tests`) are accepted automatically by `--tests-dir`.
   - It runs all `52` suites 3 times in release mode (typically ~5–30 seconds total depending on CPU speed) and ends with:
     `Correctness : 51/52 PASSED, 1 DEFERRED, 0 FAILED`

5. **Copy the printed results and basic machine info back:**
   Copy the entire output block printed by `ps2-autotest.exe` (starting from `=== PS2-AUTOTEST BENCHMARK ... ===` through `Correctness : ...`), and include these basic machine details (do not include personal identifiers like your Windows username or computer name):
   - **CPU model** (e.g., **Task Manager -> Performance -> CPU**, such as `Intel Core i7-8750H`)
   - **RAM total** (e.g., **Task Manager -> Performance -> Memory**, such as `8 GB DDR4`)
   - **Windows power mode** (e.g., *Best performance* or *Balanced*, plus OEM mode if set)
   - **Plugged in or on battery** (e.g., *Plugged in (AC)* or *On battery*)
   - *(Helpful for laptops: note whether the fans ramped hard or `Run 1` -> `Run 3` slowed down noticeably, indicating thermal throttling.)*

   Optional PowerShell snippet to print non-personal hardware/power summary fields cleanly:
   ```powershell
   Write-Output "CPU: $((Get-CimInstance Win32_Processor).Name)"
   Write-Output "RAM: $([math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1)) GB"
   Write-Output ((powercfg /getactivescheme) -replace 'Power Scheme GUID:\s*\S+\s*', 'Power Plan: ')
   $bat = Get-CimInstance Win32_Battery -ErrorAction SilentlyContinue
   if ($bat) { Write-Output "Power Source: $(if ($bat.BatteryStatus -eq 2) { 'Plugged in (AC)' } else { 'Battery / Other (' + $bat.BatteryStatus + ')' })" } else { Write-Output "Power Source: Desktop (no battery)" }
   ```
