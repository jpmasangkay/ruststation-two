# Third-Party Notices

This project includes reference constants and algorithmic attribution derived from open-source third-party software under permissive licenses as listed below.

---

## Play! (`jpd002/Play-`)

- **License**: BSD 2-Clause License
- **Repository**: <https://github.com/jpd002/Play->
- **Source file referenced**: `Source/ee/MA_VU_Lower.cpp`
- **Commit / Date read**:
  - Repository `master` HEAD commit: `1f27232772e484bfb79364b751b84f41f4c87481`
  - Latest commit touching `Source/ee/MA_VU_Lower.cpp`: `af025c07c86323fa2edaa3cba5b8a63b9a025563` (`2026-03-11`)
  - Date read: `2026-11-15`
- **Used in**: `crates/ps2-core/src/vu/efu.rs` (`ESIN`, `EEXP`, and `EATAN*` polynomial approximation constants and evaluation formulas/sequences)

### Exact License Text (`License.txt`)

```text
Copyright (c) 2006-2026, Jean-Philip Desjardins 
All rights reserved. 

Redistribution and use in source and binary forms, with or without 
modification, are permitted provided that the following conditions are met: 

 * Redistributions of source code must retain the above copyright notice, 
   this list of conditions and the following disclaimer. 
 * Redistributions in binary form must reproduce the above copyright 
   notice, this list of conditions and the following disclaimer in the 
   documentation and/or other materials provided with the distribution. 

THIS SOFTWARE IS PROVIDED BY THE AUTHOR AND CONTRIBUTORS ``AS IS'' AND ANY 
EXPRESS OR IMPLIED WARRANTIES, INCLUDING, BUT NOT LIMITED TO, THE IMPLIED 
WARRANTIES OF MERCHANTABILITY AND FITNESS FOR A PARTICULAR PURPOSE ARE 
DISCLAIMED. IN NO EVENT SHALL THE AUTHOR OR CONTRIBUTORS BE LIABLE FOR ANY 
DIRECT, INDIRECT, INCIDENTAL, SPECIAL, EXEMPLARY, OR CONSEQUENTIAL DAMAGES 
(INCLUDING, BUT NOT LIMITED TO, PROCUREMENT OF SUBSTITUTE GOODS OR 
SERVICES; LOSS OF USE, DATA, OR PROFITS; OR BUSINESS INTERRUPTION) HOWEVER 
CAUSED AND ON ANY THEORY OF LIABILITY, WHETHER IN CONTRACT, STRICT 
LIABILITY, OR TORT (INCLUDING NEGLIGENCE OR OTHERWISE) ARISING IN ANY WAY 
OUT OF THE USE OF THIS SOFTWARE, EVEN IF ADVISED OF THE POSSIBILITY OF SUCH 
DAMAGE.
```
