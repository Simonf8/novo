$pyPath = python -c "import sys; print(sys.base_exec_prefix)"
$env:PATH = "$pyPath;$env:PATH"
& "$env:USERPROFILE\.cargo\bin\cargo" run -- $args
