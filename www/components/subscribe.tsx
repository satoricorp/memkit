"use client";
import { Button } from '@/components/ui/button'
import { ArrowRight } from 'lucide-react'
import { useAuthToken } from '@convex-dev/auth/react'
import { productsConfig } from '@/config'

export default function Subscribe() {
  const token = useAuthToken()
  const isAuthenticated = Boolean(token)
  const featuredPlan =
    productsConfig.pricing.plans.find((plan) => plan.highlighted) ??
    productsConfig.pricing.plans[0]

  const handleCheckout = () => {
    if (!isAuthenticated) {
      window.location.assign('/signin')
      return
    }
    window.location.assign('/checkout/start')
  }


  return (
    <div className="min-h-screen bg-background text-foreground flex items-center justify-center p-4">
      <div className="w-full max-w-lg">
        <div className="bg-card border border-border rounded-xl p-8 relative shadow-lg">
          {/* Badge */}
          <div className="absolute -top-4 -right-4 bg-gradient-to-r from-amber-400 to-orange-400 text-white px-5 py-2 font-bold text-xs shadow-lg rounded-lg transform rotate-3 z-10">
            Featured plan
          </div>

          <div className="mb-8 pt-4">
            {/* Header */}
            <div className="mb-6">
              <h2 className="text-3xl font-bold mb-4">{featuredPlan.name}</h2>

              <div className="flex items-baseline gap-2 mb-4">
                <span className="text-6xl font-bold">{featuredPlan.price}</span>
                <span className="text-muted-foreground text-xl">
                  {featuredPlan.cadence}
                </span>
              </div>

              <div className="text-sm text-muted-foreground space-y-1 bg-muted/30 rounded-lg p-4 border border-border">
                <div className="font-medium text-foreground">
                  {featuredPlan.description}
                </div>
                <div>{productsConfig.pricing.disclaimer}</div>
              </div>
            </div>

            {/* CTA Button */}
            <Button
              className="w-full bg-gradient-to-r from-teal-400 to-cyan-400 hover:from-teal-500 hover:to-cyan-500 text-white font-bold py-7 text-lg shadow-xl hover:shadow-2xl transition-all duration-300 rounded-lg hover:scale-[1.02] active:scale-[0.98] group relative overflow-hidden"
              size="lg"
              onClick={handleCheckout}
            >
              <span className="relative z-10 flex items-center justify-center gap-2">
                {featuredPlan.cta.label}
                <ArrowRight className="w-5 h-5 transition-transform duration-300 group-hover:translate-x-1" />
              </span>
              <span className="absolute inset-0 bg-gradient-to-r from-white/0 via-white/20 to-white/0 translate-x-[-100%] group-hover:translate-x-[100%] transition-transform duration-1000"></span>
            </Button>
          </div>

          {/* Features */}
          <div className="space-y-4 mb-6">
            <h3 className="font-semibold text-base mb-4">What&apos;s included:</h3>
            <ul className="space-y-3">
              {featuredPlan.features.map((feature) => (
                <li key={feature} className="flex items-start gap-3">
                  <span className="text-cyan-400 font-bold text-lg mt-0.5">✓</span>
                  <span className="text-sm">{feature}</span>
                </li>
              ))}
            </ul>
          </div>

          {/* Footer */}
          <div className="mt-8 pt-6 border-t border-border">
            <p className="text-xs text-muted-foreground text-center leading-relaxed">
              Checkout flow is wired to Polar. Update product IDs in `.env`.
            </p>
          </div>
        </div>
      </div>
    </div>

  )
}
