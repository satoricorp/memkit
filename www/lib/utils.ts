import { clsx, type ClassValue } from "clsx"
import { twMerge } from "tailwind-merge"

export function cn(...inputs: ClassValue[]) {
  return twMerge(clsx(inputs))
}

export async function handleApiError(response: Response): Promise<never> {
  const errorText = await response.text()
  const errorData = (() => {
    try {
      return JSON.parse(errorText)
    } catch {
      return {}
    }
  })()
  throw new Error(errorData.message || `Request failed: ${response.status}`)
}
