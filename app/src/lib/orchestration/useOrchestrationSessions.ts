/**
 * Hooks backing the Connections + Agent-chat session surfaces.
 *
 * - {@link useContactSessions}: the sessions list grouped by contact agent id
 *   (the `sessionsByContact` map the roster/accordion needs), live-refreshed on
 *   the `orchestration:message` socket event.
 * - {@link useSessionTranscript}: the message transcript for one session
 *   (lazy-loaded, socket-refreshed), mapped to {@link ChatMessage} for the
 *   shared `SessionTranscript` renderer.
 *
 * Kept separate from `useOrchestrationChats` (which owns the pinned master /
 * subconscious chat surface) so a panel pulls in only what it needs.
 */
import { useCallback, useEffect, useMemo, useRef, useState } from 'react';

import { socketService } from '../../services/socketService';
import {
  orchestrationClient,
  type OrchestrationMessage,
  type OrchestrationMessageEvent,
  PaymentRequiredError,
  type SessionSummary,
} from './orchestrationClient';
import type { ChatMessage } from './useOrchestrationChats';

const TRANSCRIPT_LIMIT = 100;

export type SessionsState =
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'payment_required' }
  | { status: 'ok' };

export interface UseContactSessionsResult {
  state: SessionsState;
  /** All non-pinned session windows. */
  sessions: SessionSummary[];
  /** Sessions grouped by their peer contact agent id. */
  byContact: Map<string, SessionSummary[]>;
  refresh: () => Promise<void>;
}

function groupByContact(sessions: SessionSummary[]): Map<string, SessionSummary[]> {
  const map = new Map<string, SessionSummary[]>();
  for (const session of sessions) {
    if (session.chatKind !== 'session' || !session.agentId) continue;
    const list = map.get(session.agentId) ?? [];
    list.push(session);
    map.set(session.agentId, list);
  }
  return map;
}

export function useContactSessions(): UseContactSessionsResult {
  const [state, setState] = useState<SessionsState>({ status: 'loading' });
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const mountedRef = useRef(true);

  const refresh = useCallback(async () => {
    try {
      const { sessions: rows } = await orchestrationClient.sessionsList();
      if (!mountedRef.current) return;
      setSessions(rows.filter(s => s.chatKind === 'session'));
      setState({ status: 'ok' });
    } catch (error) {
      if (!mountedRef.current) return;
      if (error instanceof PaymentRequiredError) {
        setState({ status: 'payment_required' });
        return;
      }
      setState({
        status: 'error',
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }, []);

  useEffect(() => {
    mountedRef.current = true;
    const handle = window.setTimeout(() => void refresh(), 0);
    const onMessage = (): void => void refresh();
    socketService.on('orchestration:message', onMessage);
    socketService.on('orchestration_message', onMessage);
    return () => {
      window.clearTimeout(handle);
      mountedRef.current = false;
      socketService.off('orchestration:message', onMessage);
      socketService.off('orchestration_message', onMessage);
    };
  }, [refresh]);

  const byContact = useMemo(() => groupByContact(sessions), [sessions]);
  return { state, sessions, byContact, refresh };
}

/** OrchestrationMessage → ChatMessage view-model row. */
export function mapTranscriptMessage(message: OrchestrationMessage): ChatMessage {
  return {
    id: message.id,
    from: message.role?.trim() || message.agentId || '',
    body: message.body,
    timestamp: message.timestamp,
    encrypted: false,
    ...(message.eventKind ? { eventKind: message.eventKind } : {}),
    ...(message.toolName ? { toolName: message.toolName } : {}),
    ...(message.callId ? { callId: message.callId } : {}),
    ...(message.ok !== undefined ? { ok: message.ok } : {}),
    ...(message.isError !== undefined ? { isError: message.isError } : {}),
    ...(message.exitCode !== undefined ? { exitCode: message.exitCode } : {}),
  };
}

export type TranscriptState =
  | { status: 'idle' }
  | { status: 'loading' }
  | { status: 'error'; message: string }
  | { status: 'ok' };

export interface UseSessionTranscriptResult {
  state: TranscriptState;
  messages: ChatMessage[];
  refresh: () => Promise<void>;
}

/** Load + live-refresh one session's transcript. Pass `null` to load nothing. */
export function useSessionTranscript(sessionId: string | null): UseSessionTranscriptResult {
  const [state, setState] = useState<TranscriptState>({ status: 'idle' });
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const mountedRef = useRef(true);

  const refresh = useCallback(async () => {
    if (!sessionId) {
      setMessages([]);
      setState({ status: 'idle' });
      return;
    }
    setState(prev => (prev.status === 'ok' ? prev : { status: 'loading' }));
    try {
      const { messages: rows } = await orchestrationClient.messagesList({
        chat: sessionId,
        limit: TRANSCRIPT_LIMIT,
      });
      if (!mountedRef.current) return;
      setMessages(rows.map(mapTranscriptMessage));
      setState({ status: 'ok' });
    } catch (error) {
      if (!mountedRef.current) return;
      setState({
        status: 'error',
        message: error instanceof Error ? error.message : String(error),
      });
    }
  }, [sessionId]);

  useEffect(() => {
    mountedRef.current = true;
    const handle = window.setTimeout(() => void refresh(), 0);
    const onMessage = (payload: unknown): void => {
      const event = payload as OrchestrationMessageEvent | null;
      const affected = event && event.chatKind === 'session' ? event.sessionId : null;
      if (affected && affected === sessionId) void refresh();
    };
    socketService.on('orchestration:message', onMessage);
    socketService.on('orchestration_message', onMessage);
    return () => {
      window.clearTimeout(handle);
      mountedRef.current = false;
      socketService.off('orchestration:message', onMessage);
      socketService.off('orchestration_message', onMessage);
    };
  }, [refresh, sessionId]);

  return { state, messages, refresh };
}
