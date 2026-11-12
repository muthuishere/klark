import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { Composer, slashMatches } from '../src/components/Composer';
import { webStorage } from '../src/storage';

const cmds = ['/compact', '/config', '/context', '/cost', '/clear', '/help'].map((name) => ({ name, description: name.slice(1) }));

describe('slash popup', () => {
  it('matches only a single leading /word', () => {
    expect(slashMatches('/co', cmds).map((c) => c.name)).toEqual(['/compact', '/config', '/context', '/cost']);
    expect(slashMatches('/compact', cmds)).toEqual([]);
    expect(slashMatches('hello /co', cmds)).toEqual([]);
    expect(slashMatches('/compact now', cmds)).toEqual([]);
  });
});

describe('Composer', () => {
  const setup = (onSend = vi.fn()) => {
    render(<Composer draftKey="t" storage={webStorage(() => sessionStorage, 'klark:')} commands={cmds} onFiles={() => {}} onSend={onSend} onKey={() => {}} />);
    return { onSend, box: screen.getByLabelText('Message') as HTMLTextAreaElement };
  };
  beforeEach(() => sessionStorage.clear());
  it('opens the popup on "/" and completes with Tab', async () => {
    const { box } = setup();
    await userEvent.type(box, '/com');
    expect(screen.getByRole('listbox')).toBeVisible();
    expect(screen.getAllByRole('option')[0]).toHaveTextContent('/compact');
    await userEvent.keyboard('{Tab}');
    expect(box.value).toBe('/compact ');
    expect(screen.getByRole('listbox', { hidden: true })).not.toBeVisible();
  });
  it('Escape closes the popup, Enter then sends the text as typed', async () => {
    const { box, onSend } = setup();
    await userEvent.type(box, '/com{Escape}');
    expect(screen.getByRole('listbox', { hidden: true })).not.toBeVisible();
    await userEvent.type(box, '{Enter}');
    expect(onSend).toHaveBeenCalledWith('/com');
  });
  it('Shift+Enter makes a new line; Send button sends', async () => {
    const { box, onSend } = setup();
    await userEvent.type(box, 'a{Shift>}{Enter}{/Shift}b');
    expect(box.value).toBe('a\nb');
    await userEvent.click(screen.getByRole('button', { name: 'Send' }));
    expect(onSend).toHaveBeenCalledWith('a\nb');
    expect(box.value).toBe('');
  });
  it('keeps the draft per pane', async () => {
    const { box } = setup();
    await userEvent.type(box, 'draft');
    expect(sessionStorage.getItem('klark:draft:t')).toBe('draft');
  });
});

describe('Composer attachments', () => {
  it('takes pasted and picked images, hides attach without onFiles', async () => {
    const onFiles = vi.fn();
    const { unmount } = render(<Composer draftKey="x" onSend={() => {}} onFiles={onFiles} />);
    const img = new File(['x'], 'a.png', { type: 'image/png' });
    await userEvent.upload(screen.getByLabelText('Attach an image', { selector: 'input' }), img);
    expect(onFiles).toHaveBeenCalledWith([img]);
    unmount();
    render(<Composer draftKey="y" onSend={() => {}} />);
    expect(screen.queryByRole('button', { name: 'Attach an image' })).toBeNull();
  });
});
