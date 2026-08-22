$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$wordFixture = Join-Path $root 'sample-word.docx'
$word = $null
$document = $null
$range = $null
Remove-Item -LiteralPath $wordFixture -Force -ErrorAction SilentlyContinue
try {
    $word = New-Object -ComObject Word.Application
    $word.Visible = $false
    $word.DisplayAlerts = 0
    $document = $word.Documents.Add()
    $range = $document.Range(0, 0)
    $range.Text = 'memoria-ifilter-probe-947'
    $document.SaveAs2($wordFixture, 16)
    $document.Close(0)
    $word.Quit(0)
    Write-Host 'word_fixture=generated'
    exit 0
}
finally {
    if ($range) { [Runtime.InteropServices.Marshal]::ReleaseComObject($range) | Out-Null }
    if ($document) { [Runtime.InteropServices.Marshal]::ReleaseComObject($document) | Out-Null }
    if ($word) { [Runtime.InteropServices.Marshal]::ReleaseComObject($word) | Out-Null }
}
