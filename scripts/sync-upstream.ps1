$ErrorActionPreference = 'Stop'
function Invoke-Git {
    & git @args
    if ($LASTEXITCODE -ne 0) { throw 'Git 操作失败；保留当前状态，请检查或解决冲突后继续。' }
}
Push-Location (Split-Path $PSScriptRoot -Parent)
try {
    $status = Invoke-Git status --porcelain
    if ($status) { throw '工作区有未提交修改，请先提交或保存后再同步。' }
    if ((Invoke-Git remote) -notcontains 'upstream') { Invoke-Git remote add upstream https://github.com/SuperGness/codey.git }
    $upstreamUrl = Invoke-Git remote get-url upstream
    if ($upstreamUrl -notmatch '(?i)github[.]com[:/]SuperGness/codey([.]git)?$') { throw 'upstream 未指向官方仓库，请先核对。' }
    Invoke-Git fetch upstream master --tags
    & git show-ref --verify --quiet refs/heads/master
    if ($LASTEXITCODE -eq 0) { Invoke-Git switch master } else { Invoke-Git switch -c master upstream/master }
    Invoke-Git merge --ff-only upstream/master
    Invoke-Git push origin master
    Invoke-Git switch custom
    Invoke-Git merge --no-edit master
    Write-Host '上游已合并到 custom。请运行测试，通过后执行 git push origin custom。'
} finally { Pop-Location }
