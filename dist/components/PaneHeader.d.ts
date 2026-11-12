import type { ReactNode } from 'react';
export interface PaneHeaderProps {
    title: string;
    agent?: string;
    /** A small pill after the title (the session / workspace). */
    session?: string;
    /** working | idle | blocked | '' (drives the colour of statusText). */
    status?: string;
    statusText?: string;
    /** Tooltip of the title block (the folder). */
    cwd?: string;
    /** Buttons before the title (sidebar toggle, back). */
    leading?: ReactNode;
    /** Chat/terminal toggle; omitted when onToggleView is not given. */
    view?: 'chat' | 'terminal';
    onToggleView?: () => void;
    /** Buttons after the toggle (an overflow menu...). */
    actions?: ReactNode;
}
/** The bar above a chat: agent badge, name, session pill, live status, terminal toggle, actions. */
export declare function PaneHeader({ title, agent, session, status, statusText, cwd, leading, view, onToggleView, actions }: PaneHeaderProps): import("react").JSX.Element;
