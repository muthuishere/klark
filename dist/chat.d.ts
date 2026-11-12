import type { ChatItem } from './types';
export interface ChatRowModel {
    key: string;
    kind: 'user' | 'text' | 'tools' | 'block';
    items: ChatItem[];
    stamp: string;
}
/** Fold the flat transcript into display rows (port of ChatRowModel.Build). */
export declare function buildRows(items: ChatItem[]): ChatRowModel[];
/** Add or replace rows by id, keeping arrival order. New ids append. */
export declare function mergeItems(cur: ChatItem[], incoming: ChatItem[]): ChatItem[];
export interface ToolCall {
    key: string;
    kind: 'tool' | 'thinking';
    use?: ChatItem;
    result?: ChatItem;
}
/** Pair each tool_use with the result that follows it; thinking stands alone. */
export declare function pairCalls(items: ChatItem[]): ToolCall[];
export declare function verb(tool: string): [string, string];
export declare function toolIcon(tool: string): string;
/** "Read 3 files · Ran 2 commands · thought" */
export declare function phrase(tools: string[], thinking: number): string;
