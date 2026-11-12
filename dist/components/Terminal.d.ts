export interface TerminalHandle {
    write: (b: Uint8Array) => void;
    reset: (b: Uint8Array) => void;
    resize: (cols: number, rows: number) => void;
}
export declare function decodeBase64(b64: string): Uint8Array;
/**
 * The pane's own geometry (never fit() to the window), scaled down with CSS to the container width and
 * scrolling sideways below a minimum scale, so TUIs keep their exact layout. xterm loads only when first shown.
 */
export interface TerminalProps {
    cols: number;
    rows: number;
    visible: boolean;
    theme?: Record<string, string>;
    fontSize?: number;
}
export declare const Terminal: import("react").ForwardRefExoticComponent<TerminalProps & import("react").RefAttributes<TerminalHandle>>;
