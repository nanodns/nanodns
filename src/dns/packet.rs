//! Thin helpers around hickory-proto for building DNS responses.

use hickory_proto::op::{Message, MessageType, OpCode, ResponseCode};
use hickory_proto::rr::rdata::{A, AAAA, CNAME, MX, NS, PTR, TXT};
use hickory_proto::rr::{Name, RData, Record, RecordType};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::str::FromStr;

use crate::config::{self, DnsRecord};

/// Build a DNS NXDOMAIN response for the given query message.
pub fn nxdomain(query: &Message) -> Message {
    let mut resp = Message::new();
    resp.set_id(query.id());
    resp.set_message_type(MessageType::Response);
    resp.set_op_code(OpCode::Query);
    resp.set_authoritative(true);
    resp.set_response_code(ResponseCode::NXDomain);
    if let Some(q) = query.queries().first() {
        resp.add_query(q.clone());
    }
    resp
}

/// Build a SERVFAIL response.
pub fn servfail(query: &Message) -> Message {
    let mut resp = Message::new();
    resp.set_id(query.id());
    resp.set_message_type(MessageType::Response);
    resp.set_response_code(ResponseCode::ServFail);
    if let Some(q) = query.queries().first() {
        resp.add_query(q.clone());
    }
    resp
}

/// Convert a config DnsRecord into a hickory DNS Record, returning None on error.
pub fn to_rr(r: &DnsRecord) -> Option<Record> {
    let name = Name::from_str(&ensure_fqdn(&r.name)).ok()?;
    let ttl = r.ttl;

    let rdata = match r.record_type {
        config::RecordType::A => {
            let ip: Ipv4Addr = r.value.parse().ok()?;
            RData::A(A(ip))
        }
        config::RecordType::Aaaa => {
            let ip: Ipv6Addr = r.value.parse().ok()?;
            RData::AAAA(AAAA(ip))
        }
        config::RecordType::Cname => {
            let target = Name::from_str(&ensure_fqdn(&r.value)).ok()?;
            RData::CNAME(CNAME(target))
        }
        config::RecordType::Mx => {
            let exchange = Name::from_str(&ensure_fqdn(&r.value)).ok()?;
            RData::MX(MX::new(r.priority.unwrap_or(10), exchange))
        }
        config::RecordType::Txt => RData::TXT(TXT::new(vec![r.value.clone()])),
        config::RecordType::Ptr => {
            let target = Name::from_str(&ensure_fqdn(&r.value)).ok()?;
            RData::PTR(PTR(target))
        }
        config::RecordType::Ns => {
            let ns = Name::from_str(&ensure_fqdn(&r.value)).ok()?;
            RData::NS(NS(ns))
        }
        config::RecordType::Soa => return None, // SOA built separately
    };

    let mut rec = Record::new();
    rec.set_name(name)
        .set_ttl(ttl)
        .set_rr_type(rdata.record_type())
        .set_data(Some(rdata));
    Some(rec)
}

pub fn ensure_fqdn(name: &str) -> String {
    if name.ends_with('.') {
        name.to_string()
    } else {
        format!("{}.", name)
    }
}

/// Map config RecordType to hickory RecordType
pub fn map_qtype(rt: &config::RecordType) -> RecordType {
    match rt {
        config::RecordType::A => RecordType::A,
        config::RecordType::Aaaa => RecordType::AAAA,
        config::RecordType::Cname => RecordType::CNAME,
        config::RecordType::Mx => RecordType::MX,
        config::RecordType::Txt => RecordType::TXT,
        config::RecordType::Ptr => RecordType::PTR,
        config::RecordType::Ns => RecordType::NS,
        config::RecordType::Soa => RecordType::SOA,
    }
}
