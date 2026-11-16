import { defineCollection } from 'astro:content';
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
};
