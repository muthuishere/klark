# klark for aos

An [aos](https://github.com/muthuishere/aos) adapter: `aos klark …` runs klark
(agents, panes, messages, feeds, stats, hub), so any agent or script driving a
machine through aos can drive klark too, on macOS, Linux and Windows.

Install (klark must be on PATH):

```sh
# macOS / Linux
curl -fsSL https://raw.githubusercontent.com/muthuishere/klark/master/integrations/aos/klark.json -o "$(aos adapters path)/klark.json"
```

```powershell
# Windows
iwr https://raw.githubusercontent.com/muthuishere/klark/master/integrations/aos/klark.json -OutFile "$(aos adapters path)\klark.json"
```

macOS also: `aos klark open <folder>` (a Ghostty tab running `klark -c`):

```sh
curl -fsSL https://raw.githubusercontent.com/muthuishere/klark/master/integrations/aos/klark-open-macos.json -o "$(aos adapters path)/klark-open-macos.json"
```

On Windows open a folder's session with aos directly:
`aos tab open --app=WindowsTerminal "--cwd=C:\src\api" "--command=klark -c"`.

Then `aos klark --help`. Commands: version, status, agents, read, prompt, wait, panes,
send, msg, feeds, feed, unfeed, stats, hub. Each passes its arguments straight
to the klark command it wraps.
