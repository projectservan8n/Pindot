import { describe, expect, it } from 'vitest';
import { insertAtCaret, problemText } from './dictate';

describe('insertAtCaret', () => {
  it('fills an empty box with no spaces', () => {
    expect(insertAtCaret('', 0, 0, 'hello there')).toEqual({ text: 'hello there', caret: 11 });
  });
  it('adds a space after words that run into the new ones', () => {
    expect(insertAtCaret('Please', 6, 6, 'send it')).toEqual({ text: 'Please send it', caret: 14 });
  });
  it('does not double a space that is already there', () => {
    expect(insertAtCaret('Please ', 7, 7, 'send it').text).toBe('Please send it');
  });
  it('replaces the selection', () => {
    const r = insertAtCaret('fix the old bug now', 8, 11, 'new');
    expect(r.text).toBe('fix the new bug now');
    expect(r.caret).toBe(11);
  });
  it('keeps punctuation tight and spaces the next word', () => {
    expect(insertAtCaret('Hello.', 5, 5, 'there').text).toBe('Hello there.');
    expect(insertAtCaret('Hello world', 5, 5, 'big').text).toBe('Hello big world');
  });
  it('starts a new line cleanly', () => {
    expect(insertAtCaret('One\n', 4, 4, 'two').text).toBe('One\ntwo');
  });
  it('trims and squeezes what was said', () => {
    expect(insertAtCaret('', 0, 0, '  a   b \n c ').text).toBe('a b c');
  });
  it('ignores empty speech and a caret outside the text', () => {
    expect(insertAtCaret('abc', 1, 1, '   ')).toEqual({ text: 'abc', caret: 1 });
    expect(insertAtCaret('abc', 99, 120, 'd').text).toBe('abc d');
    expect(insertAtCaret('abc', 3, 1, 'd').text).toBe('abc d');
  });
});

describe('problemText', () => {
  it('prefers the detail from the app', () => {
    expect(problemText('no_mic', 'Microphone access is off.')).toBe('Microphone access is off.');
  });
  it('has plain words for every problem', () => {
    for (const p of ['busy', 'no_model', 'no_mic', 'nothing', 'failed']) expect(problemText(p, '').length).toBeGreaterThan(10);
  });
});
