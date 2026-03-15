import Link from "next/link";
import { copyConfig, dashboardConfig } from "@/config";
import { Card, CardContent, CardHeader, CardTitle } from "@/components/ui/card";
import { Button } from "@/components/ui/button";

export default function Dashboard() {
  return (
    <div className="mx-auto w-full max-w-6xl px-6 pb-20 pt-28">
      <div className="flex flex-col gap-3">
        <div className="text-3xl font-brand tracking-tight">
          {dashboardConfig.welcome.title}
        </div>
        <p className="text-muted-foreground max-w-2xl">
          {dashboardConfig.welcome.subtitle}
        </p>
      </div>

      <div className="mt-10 grid gap-4 md:grid-cols-3">
        {dashboardConfig.metrics.map((metric) => (
          <Card key={metric.label} className="border-border/70 bg-background/60">
            <CardHeader>
              <CardTitle className="text-sm text-muted-foreground">
                {metric.label}
              </CardTitle>
            </CardHeader>
            <CardContent className="flex items-center justify-between">
              <div className="text-2xl font-brand">{metric.value}</div>
              <span className="rounded-full bg-emerald-100 px-2 py-1 text-xs text-emerald-700">
                {metric.change}
              </span>
            </CardContent>
          </Card>
        ))}
      </div>

      <div className="mt-12 grid gap-6 lg:grid-cols-[2fr_1fr]">
        <div className="grid gap-6">
          {dashboardConfig.modules.map((module) => (
            <Card key={module.title} className="border-border/70 bg-background/60">
              <CardHeader>
                <CardTitle className="text-lg">{module.title}</CardTitle>
              </CardHeader>
              <CardContent className="space-y-4">
                <p className="text-sm text-muted-foreground">
                  {module.description}
                </p>
                <div className="flex flex-wrap gap-2">
                  {module.links.map((link) => (
                    <Button key={link.label} variant="outline" size="sm" asChild>
                      <Link href={link.href}>{link.label}</Link>
                    </Button>
                  ))}
                </div>
              </CardContent>
            </Card>
          ))}
        </div>

        <div className="space-y-6">
          <Card className="border-border/70 bg-background/60">
            <CardHeader>
              <CardTitle className="text-lg">{copyConfig.dashboard.recentActivityTitle}</CardTitle>
            </CardHeader>
            <CardContent className="space-y-4">
              {dashboardConfig.activity.map((item) => (
                <div key={item.title} className="space-y-1">
                  <div className="flex items-center justify-between">
                    <div className="text-sm font-medium">{item.title}</div>
                    <span className="text-xs text-muted-foreground">{item.time}</span>
                  </div>
                  <p className="text-xs text-muted-foreground">{item.description}</p>
                </div>
              ))}
            </CardContent>
          </Card>

          <Card className="border-border/70 bg-background/60">
            <CardHeader>
              <CardTitle className="text-lg">{copyConfig.dashboard.quickActionsTitle}</CardTitle>
            </CardHeader>
            <CardContent className="space-y-2">
              {dashboardConfig.quickActions.map((action) => (
                <Link
                  key={action.label}
                  href={action.href}
                  className="flex items-center justify-between rounded-lg border border-border/60 px-3 py-2 text-sm transition-colors hover:border-foreground/20"
                >
                  {action.label}
                  <span className="text-xs text-muted-foreground">↗</span>
                </Link>
              ))}
            </CardContent>
          </Card>
        </div>
      </div>
    </div>
  );
}
