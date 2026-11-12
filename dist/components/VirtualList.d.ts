import { type ReactNode } from 'react';
export interface VirtualHandle {
    scrollToIndex: (i: number) => void;
    scrollToBottom: () => void;
}
interface Props<T> {
    items: T[];
    getKey: (t: T) => string;
    render: (t: T, index: number) => ReactNode;
    estimate?: number;
    overscan?: number;
    className?: string;
    onPinnedChange?: (pinned: boolean) => void;
    /** Shown inside the column above the first row. */
    header?: ReactNode;
}
/**
 * Windowed chat list: only rows near the viewport are in the DOM; the rest are two spacers of the exact height
 * they would take (measured once a row has been drawn, estimated before). Stays pinned to the bottom while the
 * reader is there, so new rows follow; scrolling up unpins.
 */
declare function VirtualListInner<T>({ items, getKey, render, estimate, overscan, className, onPinnedChange, header }: Props<T>, ref: React.Ref<VirtualHandle>): import("react").JSX.Element;
export declare const VirtualList: <T>(p: Props<T> & {
    ref?: React.Ref<VirtualHandle>;
}) => ReturnType<typeof VirtualListInner>;
export {};
