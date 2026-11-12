import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { ChoiceBar, choiceKeys } from '../components/ChoiceBar';
import type { Blocked } from '../lib/types';

const agy: Blocked = {
  question: 'File access\nRead: /Users/me/notes/todo.md\nReason: outside workspace\n\nAllow access to this file?',
  choices: [{ key: '1', label: 'Yes, allow access' }, { key: '2', label: 'Yes, and always allow non-workspace access' }, { key: '3', label: 'No, deny access' }],
  selected: '1',
};

describe('choice prompts', () => {
  it('moves from the highlighted option with arrows, then enter', () => {
    expect(choiceKeys(agy, '1')).toEqual(['enter']);
    expect(choiceKeys(agy, '3')).toEqual(['down', 'down', 'enter']);
    expect(choiceKeys({ ...agy, selected: '3' }, '1')).toEqual(['up', 'up', 'enter']);
    expect(choiceKeys({ ...agy, selected: undefined }, '2')).toEqual(['2']);
    expect(choiceKeys(agy, '9')).toEqual([]);
  });
  it('renders buttons with the question and sends the keys on click', async () => {
    const sent: string[][] = [];
    render(<ChoiceBar blocked={agy} onKeys={(k) => { sent.push(k); }} />);
    expect(screen.getByText(/Allow access to this file\?/)).toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: '3. No, deny access' }));
    await userEvent.click(screen.getByRole('button', { name: 'Esc' }));
    expect(sent).toEqual([['down', 'down', 'enter'], ['esc']]);
    expect(screen.getByRole('button', { name: '1. Yes, allow access' })).toHaveClass('chosen');
  });
  it('renders nothing without choices', () => {
    const { container } = render(<ChoiceBar blocked={{ question: 'x', options: [] }} onKeys={() => {}} />);
    expect(container).toBeEmptyDOMElement();
  });
});
