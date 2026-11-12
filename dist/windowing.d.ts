export interface WindowResult {
    start: number;
    end: number;
    padTop: number;
    padBottom: number;
    total: number;
}
/** Offsets of each row's top edge, plus the total as the last element. */
export declare function offsets(count: number, heights: ReadonlyMap<string, number> | ((i: number) => number), key: (i: number) => string, estimate: number): number[];
/**
 * Which rows to draw: those that intersect [scrollTop - overscan, scrollTop + viewport + overscan].
 * Everything else is replaced by two spacers of the exact height they would have taken.
 */
export declare function computeWindow(offs: number[], scrollTop: number, viewport: number, overscan: number): WindowResult;
/** True when the viewport is within `slack` px of the bottom (so new rows should keep it pinned there). */
export declare function atBottom(scrollTop: number, viewport: number, total: number, slack?: number): boolean;
