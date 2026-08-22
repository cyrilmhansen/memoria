$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path

Set-Content -LiteralPath (Join-Path $root 'sample.txt') -Value 'memoria-ifilter-probe-947' -Encoding utf8
Set-Content -LiteralPath (Join-Path $root 'sample.html') -Value '<html><body><p>memoria-ifilter-probe-947</p></body></html>' -Encoding utf8

# Minimal text PDF. The fixture is intentionally generated, not checked in.
$pdf = @"
%PDF-1.4
1 0 obj << /Type /Catalog /Pages 2 0 R >> endobj
2 0 obj << /Type /Pages /Kids [3 0 R] /Count 1 >> endobj
3 0 obj << /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >> endobj
4 0 obj << /Type /Font /Subtype /Type1 /BaseFont /Helvetica >> endobj
5 0 obj << /Length 58 >> stream
BT /F1 12 Tf 72 720 Td (memoria-ifilter-probe-947) Tj ET
endstream endobj
xref
0 6
0000000000 65535 f
trailer << /Size 6 /Root 1 0 R >>
startxref
0
%%EOF
"@
[IO.File]::WriteAllText((Join-Path $root 'sample.pdf'), $pdf, [Text.Encoding]::ASCII)

# Minimal DOCX package with one document.xml part.
$docx = Join-Path $root 'sample.docx'
$scratch = Join-Path $root '.docx-fixture'
Remove-Item -LiteralPath $scratch -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path (Join-Path $scratch 'word') | Out-Null
Set-Content -LiteralPath (Join-Path $scratch '[Content_Types].xml') -Encoding utf8 -Value @'
<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>
'@
New-Item -ItemType Directory -Path (Join-Path $scratch '_rels') | Out-Null
Set-Content -LiteralPath (Join-Path $scratch '_rels/.rels') -Encoding utf8 -Value @'
<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>
'@
Set-Content -LiteralPath (Join-Path $scratch 'word/document.xml') -Encoding utf8 -Value @'
<?xml version="1.0" encoding="UTF-8"?><w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>memoria-ifilter-probe-947</w:t></w:r></w:p><w:sectPr/></w:body></w:document>
'@
if (Test-Path $docx) { Remove-Item -LiteralPath $docx -Force }
Add-Type -AssemblyName System.IO.Compression.FileSystem
[IO.Compression.ZipFile]::CreateFromDirectory($scratch, $docx)
Remove-Item -LiteralPath $scratch -Recurse -Force

# A real Word-generated package is kept separate from the minimal OPC fixture.
# This is only created on the controlled Windows probe host; it is never
# committed or derived from a personal document.
$wordFixture = Join-Path $root 'sample-word.docx'
$word = $null
$document = $null
Remove-Item -LiteralPath $wordFixture -Force -ErrorAction SilentlyContinue
try {
    $word = New-Object -ComObject Word.Application
    $word.Visible = $false
    $document = $word.Documents.Add()
    $range = $document.Range(0, 0)
    $range.Text = 'memoria-ifilter-probe-947'
    $document.SaveAs2($wordFixture, 16)
    $document.Close(0)
    $word.Quit(0)
    [Runtime.InteropServices.Marshal]::ReleaseComObject($range) | Out-Null
    [Runtime.InteropServices.Marshal]::ReleaseComObject($document) | Out-Null
    [Runtime.InteropServices.Marshal]::ReleaseComObject($word) | Out-Null
    Write-Host 'word_fixture=generated'
}
catch {
    if ($document) { try { $document.Close(0) } catch {} }
    if ($word) { try { $word.Quit(0) } catch {} }
    Write-Host 'word_fixture=unavailable'
}

Write-Host "Generated controlled fixtures under $root"
