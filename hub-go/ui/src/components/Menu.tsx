import { useEffect, useRef, useState, type ReactNode } from 'react';
import { IconMore } from '@muthuishere/klark-hub-ui';

/** Overflow menu: a button that opens a popup; closes on Escape, outside click, or choosing an item. */
export function Menu({ label, up, children }: { label: string; up?: boolean; children: ReactNode }) {
  const [open, setOpen] = useState(false);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (e: MouseEvent) => { if (!ref.current?.contains(e.target as Node)) setOpen(false); };
    const esc = (e: KeyboardEvent) => { if (e.key === 'Escape') setOpen(false); };
    document.addEventListener('mousedown', away);
    document.addEventListener('keydown', esc);
    return () => { document.removeEventListener('mousedown', away); document.removeEventListener('keydown', esc); };
  }, [open]);
  return (
    <div className={'menu' + (up ? ' up' : '')} ref={ref}>
      <button type="button" className="icon" aria-label={label} aria-haspopup="menu" aria-expanded={open} onClick={() => setOpen((o) => !o)}><IconMore /></button>
      {open && <div className="menu-pop" role="menu" onClick={(e) => { if ((e.target as HTMLElement).closest('a,button.menu-item')) setOpen(false); }}>{children}</div>}
    </div>
  );
}
