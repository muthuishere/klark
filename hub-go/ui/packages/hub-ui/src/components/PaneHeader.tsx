import type { ReactNode } from 'react';
import { agentIcon, agentKey } from '../panes';
import { IconChat, IconTerminal } from './Icons';

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
export function PaneHeader({ title, agent = '', session, status = '', statusText = '', cwd, leading, view = 'chat', onToggleView, actions }: PaneHeaderProps) {
  return (
    <header className="pane-bar">
      {leading}
      <div className="who" title={cwd || undefined}>
        <span className="badge-a" data-agent={agentKey(agent)} aria-hidden="true">{agentIcon(agent)}</span>
        <h1 className="name">{title}</h1>
        {session && <span className="pill">{session}</span>}
        <span className={'state state-' + (status || 'none')} role="status">{statusText}</span>
      </div>
      {onToggleView && (
        <button type="button" className={'icon tgl' + (view === 'terminal' ? ' on' : '')} aria-pressed={view === 'terminal'} aria-label="Terminal" title={view === 'terminal' ? 'Back to chat' : 'Terminal'} onClick={onToggleView}>
          {view === 'terminal' ? <IconChat /> : <IconTerminal />}
        </button>
      )}
      {actions}
    </header>
  );
}
