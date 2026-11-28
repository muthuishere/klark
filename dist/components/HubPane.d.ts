import { type ReactNode } from 'react';
import type { Transport } from '../transport';
import type { Offer, PaneInfo } from '../types';
import { type Storage } from '../storage';
import { type SlashCommand } from './Composer';
/** What the host's header menu gets: the search toggle lives in the pane, the menu in the host. */
export interface HubPaneActions {
    view: 'chat' | 'terminal';
    searchOpen: boolean;
    toggleSearch: () => void;
    agent: string;
    status: string;
    offline: boolean;
}
export interface HubPaneProps {
    transport: Transport;
    machine: string;
    pane: string;
    /** The pane from the machine's tree; undefined while it loads (the title falls back to the pane id). */
    info?: PaneInfo;
    /** The machine is connected. false: the composer is replaced by offlineText. */
    online: boolean;
    /** The pane's session (workspace) label. */
    session?: string;
    view: 'chat' | 'terminal';
    onViewChange: (v: 'chat' | 'terminal') => void;
    /** This viewer may type, press keys and upload on THIS machine (the host decides per machine). Default true. */
    canType?: boolean;
    /** Shown instead of the composer when canType is false. */
    readOnlyText?: ReactNode;
    /** Shown instead of the composer when the machine is offline. */
    offlineText?: ReactNode;
    /** Header: before the title (sidebar toggle, back link). */
    leading?: ReactNode;
    /** Header: the host's menu, given the pane's state. */
    renderActions?: (a: HubPaneActions) => ReactNode;
    /** Slash commands for the composer; default none. */
    commands?: SlashCommand[];
    /** A link in the chat: return true when the host handled it (opened it in-app). */
    onNavigate?: (href: string, a: HTMLAnchorElement) => boolean;
    /** Where an offered file downloads from; default its url as given. */
    offerHref?: (o: Offer) => string;
    /** Composer drafts; default sessionStorage under "klark:". */
    drafts?: Storage;
    /** The text a sent image becomes in front of the message; default "[image from hub] <path>". */
    imageLine?: (path: string) => string;
}
/**
 * One agent's page: header, chat or terminal, offered files, and the composer (with image upload and drop). Router
 * free: the host gives machine, pane, view and the menu. klark's own hub and hosts like the MCP Store console use it
 * over their own Transport, so the page looks and behaves the same everywhere.
 */
export declare function HubPane({ transport, machine, pane, info, online, session, view, onViewChange, canType, readOnlyText, offlineText, leading, renderActions, commands, onNavigate, offerHref, drafts, imageLine, }: HubPaneProps): import("react").JSX.Element;
