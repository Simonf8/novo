$pyPath = python -c "import sys; print(sys.base_exec_prefix)"
$env:PATH = "$pyPath;$env:PATH"
$env:PYTHONPATH = "C:\Users\kefet\AppData\Local\hermes\hermes-agent\venv\Lib\site-packages"
& "$env:USERPROFILE\.cargo\bin\cargo" run --release -- $args
