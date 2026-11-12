// @muthuishere/klark-hub-ui: the chat, terminal and sidebar of the klark hub as React components.
export type { ChatItem, ItemKind, Offer, Choice, Blocked, ChatPayload, MachineInfo, Workspace, Tab, PaneInfo, Tree, MachineTree } from './types';
export { httpTransport, TransportError, type Transport, type EventHandlers, type HttpTransportOptions } from './transport';
export { connectSSE, backoff, type SseOptions, type SseState } from './sse';
export { buildRows, mergeItems, pairCalls, phrase, verb, toolIcon, type ChatRowModel, type ToolCall } from './chat';
export { offsets, computeWindow, atBottom, type WindowResult } from './windowing';
export {
  agentIcon, agentKey, displayName, folderName, groupRows, isAgent, matches, paneAgent, paneCwd, paneId, paneStatus, statusRank, workspaceLabel,
  type Group, type Row,
} from './panes';
export { defaultStorage, memoryStorage, webStorage, type Storage } from './storage';
export { useChat, type ChatData } from './useChat';
export { ChatRow, OpenCtx, FileHrefCtx, stripTags } from './components/ChatRow';
export { ChatView, type ChatViewHandle, type ChatViewProps } from './components/ChatView';
export { ChoiceBar, choiceKeys } from './components/ChoiceBar';
export { Composer, slashMatches, type Attachment, type SlashCommand } from './components/Composer';
export { PaneHeader, type PaneHeaderProps } from './components/PaneHeader';
export { Sidebar, type SidebarProps } from './components/Sidebar';
export { Terminal, decodeBase64, type TerminalHandle, type TerminalProps } from './components/Terminal';
export { VirtualList, type VirtualHandle } from './components/VirtualList';
export { IconBack, IconChat, IconFile, IconMore, IconPlus, IconSidebar, IconTerminal, IconUp } from './components/Icons';
