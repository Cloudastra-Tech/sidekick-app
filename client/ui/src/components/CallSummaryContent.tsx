'use client';

// The structured call summary (overview, prospect, objections, qualification, action items),
// shared by the summary and call history screens. Table and section styling follows Meetily's
// summary view (MIT, see MEETILY-LICENSE.md).

import type { CallSummaryContent } from '@/lib/sidekick';

const SENTIMENT_STYLE = {
  positive: 'bg-green-100 text-green-700',
  neutral: 'bg-gray-100 text-gray-700',
  negative: 'bg-red-100 text-red-700',
} as const;

function List({ items }: { items: string[] }) {
  if (items.length === 0) return <p className="text-gray-500">Not discussed</p>;
  return (
    <ul className="list-disc pl-6 space-y-1">
      {items.map((item) => (
        <li key={item}>{item}</li>
      ))}
    </ul>
  );
}

export function summaryAsText(s: CallSummaryContent): string {
  const bullets = (items: string[]) => (items.length ? items.map((i) => `- ${i}`).join('\n') : '- Not discussed');
  return [
    `Summary: ${s.overview}`,
    `Prospect: ${s.prospect.who}. Current setup: ${s.prospect.current_setup}`,
    `Needs:\n${bullets(s.prospect.needs)}`,
    `Pain points:\n${bullets(s.pain_points)}`,
    `Objections:\n${bullets(s.objections.map((o) => `${o.objection} → ${o.response}`))}`,
    `Budget: ${s.qualification.budget}\nTimeline: ${s.qualification.timeline}\nDecision process: ${s.qualification.decision_process}`,
    `Commitments:\n${bullets(s.commitments.map((c) => `${c.owner === 'prospect' ? 'Prospect' : 'Us'}: ${c.action}`))}`,
    `Next step: ${s.next_step}`,
  ].join('\n\n');
}

export function SummaryContent({ summary }: { summary: CallSummaryContent }) {
  return (
    <div className="space-y-6 text-base text-gray-800 leading-relaxed">
      <p>
        <span className="font-semibold">Summary</span> {summary.overview}{' '}
        <span className={`ml-1 inline-block rounded-full px-2 py-0.5 text-xs align-middle ${SENTIMENT_STYLE[summary.sentiment]}`}>
          {summary.sentiment}
        </span>
      </p>

      <section className="space-y-2">
        <h3 className="font-semibold">Prospect</h3>
        <p>{summary.prospect.who}</p>
        <p className="text-gray-600">Current setup: {summary.prospect.current_setup}</p>
        <List items={summary.prospect.needs} />
      </section>

      <section className="space-y-2">
        <h3 className="font-semibold">Pain points</h3>
        <List items={summary.pain_points} />
      </section>

      <section className="space-y-2">
        <h3 className="font-semibold">Objections</h3>
        {summary.objections.length === 0 ? (
          <p className="text-gray-500">None raised</p>
        ) : (
          <table className="w-full border border-gray-200 text-sm">
            <thead>
              <tr className="bg-gray-50 text-left">
                <th className="border border-gray-200 px-3 py-2 font-semibold">Objection</th>
                <th className="border border-gray-200 px-3 py-2 font-semibold">Response given</th>
              </tr>
            </thead>
            <tbody>
              {summary.objections.map((o) => (
                <tr key={o.objection}>
                  <td className="border border-gray-200 px-3 py-2 align-top">{o.objection}</td>
                  <td className="border border-gray-200 px-3 py-2 align-top">{o.response}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <section className="space-y-2">
        <h3 className="font-semibold">Qualification</h3>
        <table className="w-full border border-gray-200 text-sm">
          <tbody>
            {[
              ['Budget', summary.qualification.budget],
              ['Timeline', summary.qualification.timeline],
              ['Decision process', summary.qualification.decision_process],
            ].map(([label, value]) => (
              <tr key={label}>
                <th className="border border-gray-200 bg-gray-50 px-3 py-2 text-left font-semibold w-40">{label}</th>
                <td className="border border-gray-200 px-3 py-2">{value}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </section>

      <section className="space-y-2">
        <h3 className="font-semibold">Action Items</h3>
        {summary.commitments.length === 0 ? (
          <p className="text-gray-500">None</p>
        ) : (
          <table className="w-full border border-gray-200 text-sm">
            <thead>
              <tr className="bg-gray-50 text-left">
                <th className="border border-gray-200 px-3 py-2 font-semibold w-32">Owner</th>
                <th className="border border-gray-200 px-3 py-2 font-semibold">Task</th>
              </tr>
            </thead>
            <tbody>
              {summary.commitments.map((c) => (
                <tr key={`${c.owner}-${c.action}`}>
                  <td className="border border-gray-200 px-3 py-2 align-top">{c.owner === 'prospect' ? 'Prospect' : 'Us'}</td>
                  <td className="border border-gray-200 px-3 py-2 align-top">{c.action}</td>
                </tr>
              ))}
            </tbody>
          </table>
        )}
      </section>

      <section className="space-y-2">
        <h3 className="font-semibold">Next step</h3>
        <p>{summary.next_step}</p>
      </section>
    </div>
  );
}
