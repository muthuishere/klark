import { type SseState } from './sse';
import type { ChatPayload, MachineInfo, Offer, Tree } from './types';
/** What a live pane/agent stream delivers. Frames come only when `frames` was asked for. */
export interface EventHandlers {
    chat?: (p: ChatPayload) => void;
    frame?: (f: {
        data: string;
        reset?: boolean;
    }) => void;
    size?: (s: {
        cols: number;
        rows: number;
    }) => void;
    offer?: (o: Offer) => void;
    state?: (s: SseState) => void;
    /** The stream reconnected: refetch what may have been missed. */
    resync?: () => void;
}
/**
 * How the components reach a hub. `m` is a machine id, `a` an agent (pane) id. Implement it over any protocol;
 * httpTransport speaks klark hub's HTTP + SSE contract.
 */
export interface Transport {
    machines(): Promise<MachineInfo[]>;
    tree(m: string): Promise<Tree>;
    getChat(m: string, a: string): Promise<ChatPayload>;
    /** Opens the live stream; returns close. */
    events(m: string, a: string, opts: {
        frames?: boolean;
    }, h: EventHandlers): () => void;
    /** Types text and presses enter. */
    send(m: string, a: string, text: string): Promise<void>;
    input(m: string, a: string, text: string): Promise<void>;
    /** Key names: enter, esc, up, down, ctrl+c ... */
    keys(m: string, a: string, keys: string[]): Promise<void>;
    /** Stores a file on the machine; the agent gets `path`. */
    upload(m: string, a: string, file: File): Promise<{
        path: string;
        name?: string;
    }>;
    /** The link for a file path an agent mentioned (line 0/undefined = none). */
    fileUrl(m: string, a: string, path: string, line?: number): string;
}
/** An HTTP error: status 0 when the hub did not answer. */
export declare class TransportError extends Error {
    status: number;
    constructor(status: number, message: string);
}
export interface HttpTransportOptions {
    fetch?: typeof fetch;
    EventSource?: typeof EventSource;
    /** Called with every response status (e.g. send the user to sign in on 401). */
    onStatus?: (status: number) => void;
}
/**
 * klark hub's contract: `{base}api/machines`, `{base}api/machines/{m}/tree`, `{base}api/machines/{m}/panes/{a}/
 * chat|events|input|keys|upload`, with the cookie. `basePath` is where the hub is mounted ("/" or "/klark/");
 * fileUrl is the hub's viewer route `{base}m/{m}/f?path=&pane=#L{line}`.
 */
export declare function httpTransport(basePath?: string, o?: HttpTransportOptions): Transport;
