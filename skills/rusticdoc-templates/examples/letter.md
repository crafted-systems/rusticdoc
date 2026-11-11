{% settings lang="en" timezone="Africa/Nairobi" %}{{ sent | date("%d %B %Y") }}

Dear {{ customer.title | default("") }}{{ customer.name }},

{% if overdue -%}
Your account shows {{ invoices | length }} unpaid invoice{{ "s" if invoices | length != 1 }}, {{ invoices | sum(attribute="amount") | currency(currency) }} in all:
{% else -%}
Thank you for paying on time. Your statement lists:
{% endif %}
{% for inv in invoices -%}
- {{ inv.number }}, due {{ inv.due | date("%d %b %Y") }}: {{ inv.amount | currency(currency) }}
{% endfor %}
{% if overdue -%}
Please pay by {{ deadline | date("%d %B %Y") }}, or reply to this letter if you think we have this wrong.
{%- endif %}

Yours sincerely,

{{ sender.name }}
{{ sender.role }}
