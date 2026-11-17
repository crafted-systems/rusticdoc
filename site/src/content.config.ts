import { defineCollection } from 'astro:content';
import { glob } from 'astro/loaders';
import { z } from 'astro/zod';
import { docsLoader } from '@astrojs/starlight/loaders';
import { docsSchema } from '@astrojs/starlight/schema';

export const collections = {
  docs: defineCollection({
    loader: docsLoader(),
    schema: docsSchema({
      extend: z.object({
        // The page's <title> and og:title when they should say more than the
        // heading does, in the words people search with. The heading and the
        // sidebar keep `title`.
        seoTitle: z.string().optional(),
      }),
    }),
  }),
  // Posts for /blog/. The file name is the address: introducing-rusticdoc.md
  // is /blog/introducing-rusticdoc/.
  blog: defineCollection({
    loader: glob({ pattern: '**/*.md', base: './src/content/blog' }),
    schema: ({ image }) => z.object({
      title: z.string(),
      // One or two sentences, used under the title, in the post list, in
      // search results and in the feed.
      description: z.string(),
      // A 1200 by 630 image (or twice that), shown on the post's card, at the
      // top of the post and as its share image. Covers are rendered from
      // scripts/blog-covers/; see the site README.
      cover: image(),
      coverAlt: z.string(),
      date: z.coerce.date(),
      author: z.string().default('The rusticdoc team'),
      // The small label above a post's title, such as Announcement or Release.
      tag: z.string().optional(),
      // A draft builds in `astro dev` but stays out of the site, the list
      // and the feed.
      draft: z.boolean().default(false),
    }),
  }),
};
