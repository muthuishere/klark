import { type MouseEvent, type ReactNode } from 'react';
import { type Group, type Row } from '../panes';
import { type Storage } from '../storage';
import type { MachineTree } from '../types';
export interface SidebarProps {
    /** Machines and their trees (Transport.machines + tree). Offline machines are listed apart. */
    items: MachineTree[];
    loaded?: boolean;
    /** The open agent, marked current. */
    current?: {
        machine: string;
        pane: string;
    };
    /** The link of an agent (or of a machine, when pane is undefined); used as the anchor's href. */
    hrefFor?: (machine: string, pane?: string) => string;
    /** A plain left click on an entry (the default navigation is prevented when given). */
    onOpen?: (machine: string, pane: string | undefined, e: MouseEvent) => void;
    /** Controlled search text; omit to keep it inside. */
    filter?: string;
    onFilterChange?: (f: string) => void;
    /** Where folded groups (`folded`) and the show-all toggle (`showall`) are remembered. Default localStorage. */
    store?: Storage;
    /** Extra content in a group's header (after its count). */
    renderGroupExtra?: (g: Group) => ReactNode;
    /** A row's second line. Default: "needs you · kind · machine · session". The "needs you" mark stays in front either way. */
    renderSubtitle?: (r: Row) => ReactNode;
    header?: ReactNode;
    /** Right side of the footer (account menu, theme...). */
    footer?: ReactNode;
    /** Shown when there are no machines. */
    empty?: ReactNode;
}
/** Agents grouped by project folder, needs-you first; search with ⌘K. */
export declare function Sidebar({ items, loaded, current, hrefFor, onOpen, filter: filterProp, onFilterChange, store, renderGroupExtra, renderSubtitle, header, footer, empty }: SidebarProps): import("react").JSX.Element;
