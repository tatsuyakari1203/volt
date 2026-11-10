import { Fragment, type ReactNode } from 'react';
import { type MessageKey, rawMessage } from './translate';

/**
 * A message with elements in its placeholders (a bold figure inside a sentence). The sentence is
 * translated as one piece, so each language puts the elements where its grammar wants them.
 */
export function tNodes(key: MessageKey, parts: Readonly<Record<string, ReactNode>>): ReactNode {
  const pieces = rawMessage(key).split(/(\{\w+\})/);
  return pieces.map((piece, index) => {
    const name = /^\{(\w+)\}$/.exec(piece)?.[1];
    const node = name === undefined ? piece : (parts[name] ?? piece);
    // The pieces never reorder, so the position is a stable key.
    // biome-ignore lint/suspicious/noArrayIndexKey: see above
    return <Fragment key={index}>{node}</Fragment>;
  });
}
