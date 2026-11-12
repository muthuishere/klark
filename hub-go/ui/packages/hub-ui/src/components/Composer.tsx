import { useEffect, useId, useMemo, useRef, useState, type ClipboardEvent, type KeyboardEvent } from 'react';
import { defaultStorage, type Storage } from '../storage';
import { IconPlus, IconUp } from './Icons';

/** An image the machine already stored: its name, the absolute path the agent gets, and a local preview URL. */
/** A command the slash popup offers. */
export interface SlashCommand { name: string; description: string }

export interface Attachment { name: string; path: string; preview?: string }
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
export function slashMatches(text: string, commands: SlashCommand[]): SlashCommand[] {
  if (!/^\/[^\s]*$/.test(text)) return [];
  const t = text.toLowerCase();
  return commands.filter((c) => c.name.toLowerCase().startsWith(t) && c.name.toLowerCase() !== t).slice(0, 60);
}

const none: never[] = [];

export function Composer({ draftKey, storage = defaultStorage, commands = none, terminal = false, attachments = none, uploading = false, onRemoveAttachment, onFiles, onSend, onKey }: Props) {
  const [text, setText] = useState(() => storage.get('draft:' + draftKey) ?? '');
  const [sel, setSel] = useState(0);
  const [dismissed, setDismissed] = useState(false);
  const ta = useRef<HTMLTextAreaElement>(null);
  const file = useRef<HTMLInputElement>(null);
  const id = useId();
  const popId = 'slash-' + id;

  useEffect(() => { storage.set('draft:' + draftKey, text || null); }, [text, draftKey, storage]);
  useEffect(() => { const t = ta.current; if (t) { t.style.height = 'auto'; t.style.height = Math.min(t.scrollHeight, 220) + 'px'; } }, [text]);

  const options = useMemo(() => (dismissed ? [] : slashMatches(text, commands)), [text, commands, dismissed]);
  const open = options.length > 0;
  const pick = (c: SlashCommand) => { setText(c.name + ' '); setSel(0); ta.current?.focus(); };
  const coarse = typeof matchMedia === 'function' && matchMedia('(pointer: coarse)').matches;

  const send = async () => {
    const t = text.trimEnd();
    if (!t && attachments.length === 0) return;
    setText('');
    setDismissed(false);
    await onSend(t);
  };
  const keydown = (e: KeyboardEvent<HTMLTextAreaElement>) => {
    if (open) {
      if (e.key === 'ArrowDown') { e.preventDefault(); setSel((s) => (s + 1) % options.length); return; }
      if (e.key === 'ArrowUp') { e.preventDefault(); setSel((s) => (s - 1 + options.length) % options.length); return; }
      if (e.key === 'Tab' || (e.key === 'Enter' && !e.shiftKey)) { e.preventDefault(); pick(options[Math.min(sel, options.length - 1)]); return; }
      if (e.key === 'Escape') { e.preventDefault(); setDismissed(true); return; }
    }
    if (e.key === 'Enter' && !e.shiftKey && !coarse && !e.nativeEvent.isComposing) { e.preventDefault(); void send(); }
  };
  const paste = (e: ClipboardEvent) => {
    const imgs = [...e.clipboardData.files].filter((f) => f.type.startsWith('image/'));
    if (imgs.length && onFiles) { e.preventDefault(); onFiles(imgs); }
  };

  return (
    <div className="composer-wrap">
      <div className="slash-pop" role="listbox" id={popId} aria-label="Commands" hidden={!open}>
        {options.map((c, i) => (
          <div key={c.name} id={`${popId}-${i}`} role="option" aria-selected={i === sel} onMouseDown={(e) => { e.preventDefault(); pick(c); }}>
            <span className="slash-name">{c.name}</span><span className="slash-desc">{c.description}</span>
          </div>
        ))}
      </div>
      <div className="composer">
        {attachments.length > 0 && (
          <div className="attachments">
            {attachments.map((a) => <span className="attachment" key={a.path} title={a.path}>{a.preview && <img className="thumb" src={a.preview} alt="" />}{a.name}<button type="button" aria-label={'Remove ' + a.name} onClick={() => onRemoveAttachment?.(a)}>×</button></span>)}
          </div>
        )}
        <textarea
          ref={ta} rows={1} value={text} aria-label="Message" aria-autocomplete="list" aria-controls={popId} aria-expanded={open}
          aria-activedescendant={open ? `${popId}-${Math.min(sel, options.length - 1)}` : undefined}
          placeholder={uploading ? 'Uploading image…' : attachments.length ? 'Add a message, or send the image' : terminal ? 'Type into the terminal' : 'Type a message'}
          onChange={(e) => { setText(e.target.value); setSel(0); setDismissed(false); }} onKeyDown={keydown} onPaste={paste}
        />
        <div className="composer-row">
          {onFiles && <>
          <input ref={file} className="hidden-file" type="file" accept="image/*" multiple tabIndex={-1} aria-label="Attach an image"
            onChange={(e) => { if (e.target.files?.length) onFiles?.([...e.target.files]); e.target.value = ''; }} />
          <button type="button" className="icon attach" title="Attach an image (paste and drop work too)" aria-label="Attach an image" onClick={() => file.current?.click()}><IconPlus /></button>
          </>}
          {terminal && <><button type="button" className="key" title="Esc" onClick={() => onKey?.('esc')}>esc</button><button type="button" className="key" title="Ctrl-C" onClick={() => onKey?.('ctrl+c')}>^C</button></>}
          <span className="grow" />
          <button type="button" className="send" aria-label="Send" onClick={() => void send()}><IconUp /></button>
        </div>
      </div>
    </div>
  );
}
