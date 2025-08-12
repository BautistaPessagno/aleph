import { generateText } from 'ai';
import { z } from 'zod';
import { promises as fs } from 'fs';
import path from 'path';

/**
 * Tools available to the agent.
 */
export const tools = {
  summary: {
    description: 'Summarize the contents of a file.',
    parameters: z.object({
      filePath: z.string(),
    }),
    execute: async ({ filePath }: { filePath: string }) => {
      const content = await fs.readFile(filePath, 'utf8');
      const { text } = await generateText({
        model: 'openai:gpt-4o-mini',
        prompt: `Summarize the following text:\n\n${content}`,
      });
      return text;
    },
  },
  createFile: {
    description: 'Create a new file with a basic module structure.',
    parameters: z.object({
      filePath: z.string(),
      content: z.string().optional(),
    }),
    execute: async ({
      filePath,
      content,
    }: {
      filePath: string;
      content?: string;
    }) => {
      const initialContent =
        content ?? `// ${path.basename(filePath)}\n\nexport {};\n`;
      await fs.writeFile(filePath, initialContent, 'utf8');
      return `Created file at ${filePath}`;
    },
  },
};

/**
 * Run the agent with the provided prompt. The agent can call the tools defined
 * above when generating its response.
 */
export async function runAgent(prompt: string) {
  const result = await generateText({
    model: 'openai:gpt-4o-mini',
    tools,
    prompt,
  });
  return result.text;
}

export type AgentTools = typeof tools;
