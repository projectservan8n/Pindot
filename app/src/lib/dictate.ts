// Pure helpers for dictation into the chat box (unit tested in dictate.test.ts).

export interface Inserted {
  text: string;
  /** Where the caret goes: just after what was inserted. */
  caret: number;
}

const OPENERS = /[\s(\[{"'“‘]$/;
const CLOSERS = /^[\s.,;:!?)\]}"'”’]/;

/**
 * Put dictated words into `text` at the caret, replacing the selection
 * (start..end). A space goes on each side only where one is needed, so
 * "hello" + "there" gives "hello there", and punctuation is not pushed away.
 */
export function insertAtCaret(text: string, start: number, end: number, spoken: string): Inserted {
  const said = spoken.trim().replace(/\s+/g, ' ');
  const a = Math.max(0, Math.min(start, text.length));
  const b = Math.max(a, Math.min(end, text.length));
  if (!said) return { text, caret: a };
  const before = text.slice(0, a);
  const after = text.slice(b);
  const lead = before === '' || OPENERS.test(before) ? '' : ' ';
  const tail = after === '' || CLOSERS.test(after) ? '' : ' ';
  const middle = `${lead}${said}${tail}`;
  // The caret sits after the words, before the space that was added for what follows.
  return { text: before + middle + after, caret: before.length + lead.length + said.length + tail.length };
}

export type Problem = 'busy' | 'no_model' | 'no_mic' | 'nothing' | 'failed';

/** Plain words for a dictation problem. `message` is the app's own detail. */
export function problemText(problem: Problem | string, message: string): string {
  if (message) return message;
  switch (problem) {
    case 'no_model':
      return 'There is no speech model yet. Download it under Setup.';
    case 'no_mic':
      return 'The microphone is not available. Check it in Settings.';
    case 'busy':
      return 'The microphone is busy. Try again in a moment.';
    case 'nothing':
      return 'I did not catch any words. Try again.';
    default:
      return 'Dictation did not work. Try again.';
  }
}
