import type { Blocked } from '../types';
/** Keys that pick `key` in an arrow-selected menu: from the highlighted option, up/down then enter. Without a
 * known highlight, the digit itself (claude, codex and agy menus all take it). */
export declare function choiceKeys(b: Blocked, key: string): string[];
/** The agent's choice menu as buttons under the last message. */
export declare function ChoiceBar({ blocked, onKeys }: {
    blocked: Blocked;
    onKeys: (keys: string[]) => Promise<unknown> | void;
}): import("react").JSX.Element | null;
