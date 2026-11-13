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

Then `aos klark --help`. Commands: status, agents, read, prompt, wait, panes,
send, msg, feeds, feed, unfeed, stats, hub. Each passes its arguments straight
to the klark command it wraps.
