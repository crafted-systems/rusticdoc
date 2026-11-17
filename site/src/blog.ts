// What the blog's pages and its feed share: the published posts, newest
// first, and how a post's date and length are shown.
import { getCollection, type CollectionEntry } from 'astro:content';

export type Post = CollectionEntry<'blog'>;

export async function posts(): Promise<Post[]> {
  const all = await getCollection('blog', (p) => import.meta.env.DEV || !p.data.draft);
  return all.sort((a, b) => b.data.date.getTime() - a.data.date.getTime());
}

/** The year a post was published, which is part of its address. */
export const postYear = (post: Post) => String(post.data.date.getUTCFullYear());

/** /blog/2026/introducing-rusticdoc/: the year it was published, then the file name. */
export const postPath = (post: Post) => `/blog/${postYear(post)}/${post.id}/`;

/** 17 November 2026 */
export const longDate = (date: Date) =>
  date.toLocaleDateString('en-GB', { day: 'numeric', month: 'long', year: 'numeric', timeZone: 'UTC' });

export const TEAM = 'The rusticdoc team';

/** VM for Vincent Mosoti; empty for the team, which shows the logo instead. */
export const initials = (author: string) =>
  author === TEAM ? '' : author.split(/\s+/).map((w) => w[0]).join('').slice(0, 2).toUpperCase();

/** Minutes to read, at about 220 words a minute, code included. */
export function readingTime(post: Post): number {
  const words = (post.body ?? '').split(/\s+/).filter(Boolean).length;
  return Math.max(1, Math.round(words / 220));
}
