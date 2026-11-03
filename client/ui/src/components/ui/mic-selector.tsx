'use client';

// Adapted from ElevenLabs UI (MIT, see ELEVENLABS-UI-LICENSE.md): `mic-selector` component.
// Sidekick changes: devices come from Windows (WASAPI) through the backend instead of the
// browser, so the webview never opens the mic (no getUserMedia before consent) and the chosen
// mic is the one capture actually records. No mute toggle or live waveform preview, since both
// needed a browser mic stream. A "Windows default" entry keeps following the system setting.

import { useCallback, useEffect, useState } from 'react';
import { Check, ChevronsUpDown, Mic } from 'lucide-react';
import { toast } from 'sonner';

import { cn } from '@/lib/utils';
import { backend, type InputDevice } from '@/lib/sidekick';
import { Button } from '@/components/ui/button';
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from '@/components/ui/dropdown-menu';

export interface MicSelectorProps {
  disabled?: boolean;
  className?: string;
}

export function MicSelector({ disabled, className }: MicSelectorProps) {
  const { devices, chosen, loading, error, reload, choose } = useInputDevices();

  const defaultDevice = devices.find((d) => d.is_default);
  const chosenDevice = chosen ? devices.find((d) => d.id === chosen) : undefined;
  const label = loading && devices.length === 0
    ? 'Loading...'
    : chosenDevice?.name ?? (chosen ? 'Chosen mic unplugged' : defaultDevice?.name ?? 'Default mic');

  return (
    <DropdownMenu onOpenChange={(open) => open && reload()}>
      <DropdownMenuTrigger asChild>
        <Button
          variant="ghost"
          size="sm"
          className={cn('hover:bg-accent flex w-44 min-w-0 shrink cursor-pointer items-center gap-1.5', className)}
          disabled={disabled}
          title={disabled ? 'Change the mic between calls' : 'Microphone for your side of the call'}
        >
          <Mic className="h-4 w-4 flex-shrink-0" />
          <span className="min-w-0 flex-1 truncate text-left text-xs">{label}</span>
          <ChevronsUpDown className="h-3 w-3 flex-shrink-0" />
        </Button>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="center" side="top" className="w-72">
        <DropdownMenuLabel className="text-xs font-normal text-muted-foreground">Your microphone</DropdownMenuLabel>
        {error ? (
          <DropdownMenuItem disabled>{error}</DropdownMenuItem>
        ) : (
          <>
            <DropdownMenuItem onSelect={() => choose(null)} className="flex items-center justify-between">
              <span className="truncate">
                Windows default{defaultDevice ? ` (${defaultDevice.name})` : ''}
              </span>
              {!chosen && <Check className="h-4 w-4 flex-shrink-0" />}
            </DropdownMenuItem>
            <DropdownMenuSeparator />
            {devices.length === 0 && !loading && <DropdownMenuItem disabled>No microphones found</DropdownMenuItem>}
            {devices.map((device) => (
              <DropdownMenuItem
                key={device.id}
                onSelect={() => choose(device.id)}
                className="flex items-center justify-between"
              >
                <span className="truncate">{device.name}</span>
                {chosen === device.id && <Check className="h-4 w-4 flex-shrink-0" />}
              </DropdownMenuItem>
            ))}
          </>
        )}
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

export function useInputDevices() {
  const [devices, setDevices] = useState<InputDevice[]>([]);
  const [chosen, setChosen] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  const reload = useCallback(async () => {
    setLoading(true);
    try {
      const result = await backend.listMics();
      setDevices(result.devices);
      setChosen(result.chosen);
      setError(null);
    } catch (err) {
      setError(String(err));
    } finally {
      setLoading(false);
    }
  }, []);

  const choose = useCallback(async (id: string | null) => {
    const previous = chosen;
    setChosen(id);
    try {
      await backend.setMic(id);
    } catch (err) {
      setChosen(previous);
      toast.error(String(err));
    }
  }, [chosen]);

  useEffect(() => {
    reload();
  }, [reload]);

  return { devices, chosen, loading, error, reload, choose };
}
