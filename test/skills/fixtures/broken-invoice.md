# Invoice {{ number }}

Billed to: {{ customer.fullname }}
Date: {{ issued | date("%d %B %Y") }}

| Item | Qty | Amount |
|---|---|---|
{% for i in items %}| {{ i.name }} | {{ i.qty }} | {{ i.qty * i.price | currency(currency) }} |

**Total: {{ total | money(currency) }}**
