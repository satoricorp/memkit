export type HasPlanResponse = {
  success: boolean
  hasActiveSubscription?: boolean
}

export type UserCreationRequest = {
  auth_id: string
  org_name?: string
  name?: string
  email?: string
  first_name?: string
  last_name?: string
  username?: string
  image_url?: string
}

export type UserCreationResponse = {
  api_key: string
  message?: string
}