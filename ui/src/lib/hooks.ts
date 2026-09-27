/**
 * Shared React hooks.
 *
 * They live here rather than inside the component that first needed them: both the job
 * dashboard and the translate page show elapsed times, and neither should own the other's
 * copy.
 */

import { useEffect, useState } from "react";

/**
 * Re-renders on an interval while `active`, so elapsed times and ETAs keep moving without a
 * backend event. Inactive means "nothing is running": the interval is cleared and the clock
 * simply stops, instead of waking the UI every second for a number nobody reads.
 */
export function useTicker(active: boolean, intervalMs = 1000): number {
  const [now, setNow] = useState<number>(() => Date.now());

  useEffect(() => {
    if (!active) {
      return;
    }
    const timer = window.setInterval(() => {
      setNow(Date.now());
    }, intervalMs);
    return () => {
      window.clearInterval(timer);
    };
  }, [active, intervalMs]);

  return now;
}
