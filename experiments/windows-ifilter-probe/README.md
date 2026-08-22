# Windows IFilter probe

This is a standalone helper/probe, not a workspace member and not a Memoria
runtime dependency. On Windows, generate controlled fixtures first:

```powershell
powershell -ExecutionPolicy Bypass -File .\fixtures\generate-fixtures.ps1
.\target\release\windows-ifilter-probe.exe .\fixtures\sample.txt
.\target\release\windows-ifilter-probe.exe .\fixtures\sample.pdf
.\target\release\windows-ifilter-probe.exe .\fixtures\sample.docx
```

The expected phrase is `memoria-ifilter-probe-947`. The probe writes extracted
UTF-8 text to stdout and aggregate status/timing diagnostics to stderr. It
does not print document contents in diagnostics. A missing registered handler
is reported as `unsupported`; a handler that loads but fails while filtering
is reported as `failed`.

The parent process must enforce the 10-second wall-clock timeout and the
8 MiB stdout bound by killing and waiting for this child. The probe itself
also refuses inputs above 64 MiB and stops accumulating output above 8 MiB.
