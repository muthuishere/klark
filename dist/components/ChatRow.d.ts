import { type ChatRowModel } from '../chat';
/** Which <details> are open, kept outside the rows so a row scrolled out of the window and back keeps its state. */
export declare const OpenCtx: import("react").Context<{
    has: (k: string) => boolean;
    set: (k: string, open: boolean) => void;
}>;
/** How a tool row links a file path (e.g. transport.fileUrl bound to the pane); without it the path is plain text. */
export declare const FileHrefCtx: import("react").Context<((path: string, line?: number) => string) | null>;
declare function ChatRowImpl({ row }: {
    row: ChatRowModel;
}): import("react").JSX.Element;
export declare function stripTags(html?: string): string;
/** A row re-renders only when its content (stamp) changes. */
export declare const ChatRow: import("react").MemoExoticComponent<typeof ChatRowImpl>;
export {};
