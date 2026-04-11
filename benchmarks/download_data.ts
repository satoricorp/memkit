import { mkdir, writeFile } from "fs/promises";
import { existsSync } from "fs";

const DATA_DIR = "./data";

const FILES = [
  {
    name: "longmemeval_s_cleaned.json",
    url: "https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_s_cleaned.json",
  },
  {
    name: "longmemeval_oracle.json",
    url: "https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_oracle.json",
  },
];

async function downloadFile(url: string, dest: string): Promise<void> {
  console.log(`Downloading ${url}...`);
  const response = await fetch(url);
  if (!response.ok) {
    throw new Error(`Failed to download ${url}: ${response.statusText}`);
  }
  const content = await response.text();
  await writeFile(dest, content);
  console.log(`Saved to ${dest}`);
}

async function main() {
  if (!existsSync(DATA_DIR)) {
    await mkdir(DATA_DIR, { recursive: true });
  }

  for (const file of FILES) {
    const dest = `${DATA_DIR}/${file.name}`;
    if (existsSync(dest)) {
      console.log(`${dest} already exists, skipping download`);
      continue;
    }
    await downloadFile(file.url, dest);
  }

  console.log("Download complete!");
}

main().catch(console.error);
