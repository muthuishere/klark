import { type Storage } from '../storage';
/** An image the machine already stored: its name, the absolute path the agent gets, and a local preview URL. */
/** A command the slash popup offers. */
export interface SlashCommand {
    name: string;
    description: string;
}
export interface Attachment {
    name: string;
    path: string;
    preview?: string;
}
interface Props {
    /** The draft survives reloads under `draft:` + draftKey in `storage` (default localStorage). */
    draftKey: string;
    storage?: Storage;
    commands?: SlashCommand[];
    /** Shows esc / ^C keys and the terminal placeholder. */
    terminal?: boolean;
    attachments?: Attachment[];
    uploading?: boolean;
    onRemoveAttachment?: (a: Attachment) => void;
    /** Images attached, pasted or dropped on the composer; omit to hide the attach button. */
    onFiles?: (files: File[]) => void;
    onSend: (text: string) => Promise<void> | void;
    onKey?: (key: string) => void;
}
/** The slash popup lists the commands that start with what is typed, only while the text is a single "/word". */
export declare function slashMatches(text: string, commands: SlashCommand[]): SlashCommand[];
export declare function Composer({ draftKey, storage, commands, terminal, attachments, uploading, onRemoveAttachment, onFiles, onSend, onKey }: Props): import("react").JSX.Element;
export {};
