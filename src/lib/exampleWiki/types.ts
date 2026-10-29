// SPDX-License-Identifier: AGPL-3.0-only
/** One string in each app language. */
export interface ExampleText {
  readonly en: string;
  readonly "zh-Hant": string;
  readonly "zh-Hans": string;
}

/** Where a source came from: meeting notes, an AI chat, a coding session,
 *  a PDF, a web page, or an interview note. */
export type ExampleSourceKind = "notes" | "chat" | "session" | "pdf" | "web" | "interview";

export interface ExampleSource {
  readonly id: string;
  readonly kind: ExampleSourceKind;
  readonly agent: string;
  readonly space: string;
  /** Day of October 2026 it was written. */
  readonly day: number;
  readonly title: ExampleText;
  readonly body: ExampleText;
}

export interface ExampleSentence {
  readonly text: ExampleText;
  readonly sources: readonly string[];
  /** Whether the cited source backs the sentence; null for a line with no source. */
  readonly verified: boolean | null;
}

export interface ExampleWikiContent {
  readonly sources: readonly ExampleSource[];
  readonly pages: readonly {
    readonly id: string;
    readonly space: string;
    readonly title: ExampleText;
    readonly sections: readonly { readonly heading: ExampleText | null; readonly sentences: readonly ExampleSentence[] }[];
  }[];
  readonly spaces: readonly { readonly id: string; readonly name: ExampleText; readonly description: ExampleText }[];
  readonly entities: readonly { readonly id: string; readonly type: string; readonly name: string; readonly pages: readonly string[] }[];
}
