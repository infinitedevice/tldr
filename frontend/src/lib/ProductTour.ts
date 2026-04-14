// SPDX-FileCopyrightText: 2026 Martin Donnelly
// SPDX-FileCopyrightText: 2026 Collabora Ltd.
// SPDX-License-Identifier: MIT OR Apache-2.0

/**
 * Product tour powered by Shepherd.js.
 *
 * `startTour()` builds and immediately starts a guided tour of the UI.
 * `startTourIfNew()` only starts it if the user has not seen it before
 * (checked via `localStorage['tldr-tour-seen']`).
 *
 * Update the `STEPS` array in this file whenever a new user-facing feature
 * is added to the web UI (see AGENTS.md).
 */

import Shepherd, { type Tour } from 'shepherd.js';
import 'shepherd.js/dist/css/shepherd.css';

const SEEN_KEY = 'tldr-tour-seen';

// Steps are defined inside buildTour() so the `tour` variable is in direct closure
// scope for the skip guards — avoids the brittle `(this as any).tour` pattern.
// Update this list whenever a new user-facing feature is added (see AGENTS.md).
function buildTour(): Tour {
  const tour = new Shepherd.Tour({
    useModalOverlay: true,
    defaultStepOptions: {
      cancelIcon: { enabled: true },
      classes: 'tldr-tour-step',
      scrollTo: { behavior: 'smooth', block: 'center' },
      buttons: [
        {
          text: 'Back',
          action() { tour.back(); },
          classes: 'shepherd-button-secondary',
        },
        {
          text: 'Next →',
          action() { tour.next(); },
        },
      ],
    },
  });

  // Steps with skipIfAbsent: true generate a when.show() guard that advances
  // past the step if the target element is not in the DOM (e.g. no summaries loaded).
  const steps: Array<{
    id: string;
    attachTo: { element: string; on: 'bottom' | 'top' };
    title: string;
    text: string;
    skipIfAbsent?: true;
  }> = [
      {
        id: 'summarise',
        attachTo: { element: '[data-tour="summarise-btn"]', on: 'bottom' },
        title: 'Live summaries',
        text: 'Summaries are generated automatically in the background. They appear and update here in real time — no button needed.',
        skipIfAbsent: true,
      },
      {
        id: 'settings',
        attachTo: { element: '[data-tour="settings-btn"]', on: 'bottom' },
        title: 'User menu',
        text: 'Click the avatar to open the user menu — configure <strong>Priority Users</strong>, <strong>Your Role</strong> for Insights, and <strong>Configuration</strong>. The <strong>Help</strong> option restarts this tour.',
      },
      {
        id: 'channel-card',
        attachTo: { element: '[data-tour="channel-card"]:first-of-type', on: 'top' },
        title: 'Channel cards',
        text: 'Each card shows a channel with its unread and mention counts. Click the channel name to open it in Mattermost.',
        skipIfAbsent: true,
      },
      {
        id: 'summary-html',
        attachTo: { element: '[data-tour="summary-html"]:first-of-type', on: 'top' },
        title: 'AI summary',
        text: 'The AI-generated summary uses markdown bullet points. Context messages (already read) inform the summary but are not listed.',
        skipIfAbsent: true,
      },
      {
        id: 'action-items',
        attachTo: { element: '[data-tour="action-items"]:first-of-type', on: 'top' },
        title: 'Action items',
        text: 'Action items extracted by the AI. Mark them <em>Done</em> (resolved) or <em>Ignore</em> to hide them. IDs are stable so duplicates are deduplicated across runs.',
        skipIfAbsent: true,
      },
      {
        id: 'mark-read',
        attachTo: { element: '[data-tour="mark-read"]:first-of-type', on: 'top' },
        title: 'Mark as read',
        text: '<strong>Mark as read</strong> advances the channel watermark to now. The channel will be excluded from the next summarise run until new messages arrive.',
        skipIfAbsent: true,
      },
    ];

  steps.forEach((step, i) => {
    const isLast = i === steps.length - 1;
    const def: Parameters<typeof tour.addStep>[0] = {
      id: step.id,
      attachTo: step.attachTo,
      title: step.title,
      text: step.text,
    };
    if (step.skipIfAbsent) {
      const selector = step.attachTo.element;
      def.when = {
        show() {
          if (!document.querySelector(selector)) tour.next();
        },
      };
    }
    if (isLast) {
      def.buttons = [
        { text: 'Back', action() { tour.back(); }, classes: 'shepherd-button-secondary' },
        { text: 'Done ✓', action() { tour.complete(); } },
      ];
    }
    tour.addStep(def);
  });

  tour.on('complete', () => localStorage.setItem(SEEN_KEY, '1'));
  tour.on('cancel', () => localStorage.setItem(SEEN_KEY, '1'));

  return tour;
}

export function startTour() {
  buildTour().start();
}

export function startTourIfNew() {
  if (!localStorage.getItem(SEEN_KEY)) {
    startTour();
  }
}
