import { type ReactNode } from 'react';
import type { Blocked, ChatItem } from '../types';
export interface ChatViewProps {
    items: ChatItem[];
    loading?: boolean;
    error?: string | null;
    blocked?: Blocked | null;
    /** working shows a "working…" line. */
    status?: string;
    /** Hides the choice bar and the working line. */
    offline?: boolean;
    /** Answer a choice menu (key names). */
    onKeys?: (keys: string[]) => Promise<unknown> | void;
    /** Links of file paths in tool rows (server-rendered paths come linked already). */
    fileHref?: (path: string, line?: number) => string;
    /**
     * A plain left click on a link inside the chat (file paths, `a.fpath`). Return true to say it was handled
     * (the browser's navigation is then prevented).
     */
    onNavigate?: (href: string, a: HTMLAnchorElement) => boolean | void;
    /** Search bar above the chat. */
    searchOpen?: boolean;
    onSearchClose?: () => void;
    /** Below the list, above the choice bar (e.g. files the agent offered). */
    footer?: ReactNode;
    className?: string;
}
export interface ChatViewHandle {
    scrollToBottom: () => void;
}
/** A windowed chat: rows, tool groups, a choice bar, search, "jump to latest". */
export declare const ChatView: import("react").ForwardRefExoticComponent<ChatViewProps & import("react").RefAttributes<ChatViewHandle>>;
