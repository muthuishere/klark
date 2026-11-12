# @muthuishere/klark-hub-ui

React components for watching and driving coding agents (Claude Code, Codex, Devin, ...): the chat view, composer,
terminal and sidebar of the [klark](https://github.com/muthuishere/klark) hub. They talk to a hub through a small
`Transport` interface; `httpTransport` speaks klark hub's HTTP + SSE contract (`hub-go/API.md`). The Go side of
the same rows is `github.com/muthuishere/klark/hub-go/chat` (and `/feed`).

## Install

From the GitHub tag (the tag's root is the built package; nothing runs at install):

```sh
npm install github:muthuishere/klark#hub-ui-v0.1.0 @xterm/xterm @xterm/addon-unicode11
```

Peers: `react >= 18`, `@xterm/xterm ^6`, `@xterm/addon-unicode11` (xterm loads only when a Terminal is first shown).
No router: links are plain `href`s plus `onOpen` / `onNavigate` callbacks.

## Use

```tsx
import { ChatView, Composer, PaneHeader, Sidebar, httpTransport, useChat, type MachineTree } from '@muthuishere/klark-hub-ui';
import '@muthuishere/klark-hub-ui/style.css';

const hub = httpTransport('/'); // where the hub is mounted

function Agent({ machine, pane }: { machine: string; pane: string }) {
  const chat = useChat(hub, machine, pane);
  return (
    <div className="khu" style={{ display: 'flex', flexDirection: 'column', height: '100vh' }}>
      <PaneHeader title={pane} agent="claude" status={chat.blocked ? 'blocked' : ''} />
      <ChatView items={chat.items} loading={chat.loading} error={chat.error} blocked={chat.blocked}
        onKeys={(keys) => hub.keys(machine, pane, keys)}
        fileHref={(path, line) => hub.fileUrl(machine, pane, path, line)} />
      <Composer draftKey={machine + '/' + pane} onSend={(text) => hub.send(machine, pane, text)}
        onFiles={async (files) => { for (const f of files) await hub.upload(machine, pane, f); }} />
    </div>
  );
}
```

`Sidebar` lists `MachineTree[]` (from `transport.machines()` + `transport.tree(id)`), grouped by project folder with
blocked agents first: props `items`, `current`, `hrefFor`, `onOpen`, `filter`/`onFilterChange`, `store`,
`renderGroupExtra`, `header`, `footer`. `Terminal` (a `ref` with `write` / `reset` / `resize`) draws the pane at its own
geometry; pass it as the last argument of `useChat(hub, m, a, true, termRef)` to stream frames into it.

## Exports

- Transport: `Transport`, `EventHandlers`, `httpTransport(basePath, {fetch?, EventSource?, onStatus?})`, `TransportError`
- Components: `Sidebar`, `ChatView`, `ChatRow`, `VirtualList`, `Composer`, `Terminal`, `PaneHeader`, `ChoiceBar`, icons
- Hooks and helpers: `useChat`, `connectSSE`, `backoff`, `buildRows`, `mergeItems`, `pairCalls`, `phrase`, `choiceKeys`,
  `slashMatches`, pane helpers (`displayName`, `groupRows`, `paneStatus`, ...)
- Storage: `Storage` (`get`/`set`), `webStorage(area?, prefix?)`, `memoryStorage()`, `defaultStorage` (localStorage)
- Types: `ChatItem`, `Blocked`, `ChatPayload`, `Offer`, `MachineInfo`, `PaneInfo`, `Tree`, `MachineTree`, ...

## Styling

`style.css` styles the components under any ancestor with class `khu`, with zero specificity (`:where(.khu)`), so
your own CSS wins. It sets no global resets and is themed only through CSS variables, each with a light fallback:
`--bg --fg --muted --line --hover --card --side --accent --on-accent --link --bubble --code-bg --code-head --out-bg
--out-fg --blocked --working --idle --ok --bad --skel --mark --mono --serif --hl-*`.

## Security

Chat `html` is rendered by the hub (markdown with raw HTML dropped, only http/https/mailto links, ANSI as classes)
and inserted as is. Use a Transport whose server sanitises the same way (the Go `chat` package does).

## License

Apache-2.0
