"""Read-only canonical value-label audit; bounded SQLite working aggregation."""
import argparse
import hashlib
import json
import math
import sqlite3
import time
from pathlib import Path

from checkpoint import load_checkpoint
from common import eligible_label, validate_split_manifest
from compact import partitions
from dataset import open_dataset, file_hash
from train_value_only import json_hash


def connect(path,cache_mib=32,disk_mib=2048):
    if cache_mib<1 or disk_mib<8:raise ValueError('invalid SQLite resource budget')
    db=sqlite3.connect(path)
    db.row_factory=sqlite3.Row
    db.execute('PRAGMA page_size=4096')
    db.execute(f'PRAGMA cache_size=-{cache_mib*1024}')
    db.execute('PRAGMA temp_store=FILE')
    db.execute(f'PRAGMA max_page_count={disk_mib*1024*1024//4096}')
    db.execute(f'PRAGMA temp.cache_size=-{cache_mib*1024}')
    db.execute(f'PRAGMA temp.max_page_count={disk_mib*1024*1024//4096}')
    db.executescript('''
      CREATE TABLE records(idx INTEGER PRIMARY KEY, split TEXT NOT NULL, key BLOB NOT NULL,
        source INTEGER, exact INTEGER, raw INTEGER, q REAL, policy INTEGER, depth INTEGER,
        requested INTEGER, termination INTEGER, work INTEGER, game TEXT, trajectory TEXT,
        lineage TEXT, qualified INTEGER, sample_order BLOB);
      CREATE TABLE position_stats(scope TEXT, subset TEXT, key BLOB, n INTEGER,
        raw_min INTEGER,raw_max INTEGER,raw_mean REAL,raw_std REAL,q_min REAL,q_max REAL,
        q_mean REAL,q_std REAL,depth_min INTEGER,depth_max INTEGER,policy_count INTEGER,
        PRIMARY KEY(scope,subset,key)) WITHOUT ROWID;
      CREATE VIEW duplicate_positions AS SELECT *,raw_max-raw_min AS raw_range,
        q_max-q_min AS q_range FROM position_stats;
    ''')
    return db


def ingest(db,records,split,scale,qualified=None,seed=17):
    """Stream (dataset index, record); tests and real corpus use identical logic."""
    if scale<=0:raise ValueError('score scale must be positive')
    count=0
    for index,r in records:
        if not eligible_label(r):continue
        q=float((r.value>0)-(r.value<0)) if r.exact else r.value/(scale+abs(r.value))
        priority=hashlib.sha256(f'{seed}:{index}'.encode()).digest()
        db.execute('INSERT INTO records VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)',
            (index,split,r.position_key,r.source,int(r.exact),r.value,q,r.policy_move,
             r.completed_depth,r.requested_depth,r.termination,r.work,str(r.game_id),
             r.trajectory_id,r.lineage_id,int(qualified(index) if qualified else True),priority))
        count+=1
        if count%10000==0:db.commit()
    db.commit()


def quantiles(db,query,params=(),probabilities=(.5,.75,.9,.95,.99,1.)):
    n=db.execute(f'SELECT count(*) FROM ({query})',params).fetchone()[0]
    if not n:return None
    result={}
    for p in probabilities:
        at=p*(n-1);lo=math.floor(at);hi=math.ceil(at)
        values=[row[0] for row in db.execute(f'SELECT * FROM ({query}) ORDER BY 1 LIMIT ? OFFSET ?',(*params,hi-lo+1,lo))]
        result[str(p)]=values[0]+(values[-1]-values[0])*(at-lo)
    return result


def distribution(db,where,params,column):
    return [dict(row) for row in db.execute(f'SELECT {column} AS value,count(*) AS occurrences,count(DISTINCT key) AS positions FROM records WHERE {where} GROUP BY {column} ORDER BY {column}',params)]


def consistency(db,scope,subset,where,params=()):
    # All large grouped intermediates live in bounded SQLite, never a key->rows dict.
    db.executescript('DROP TABLE IF EXISTS temp.r; DROP TABLE IF EXISTS temp.g; DROP TABLE IF EXISTS temp.m; DROP TABLE IF EXISTS temp.h;')
    db.execute(f'CREATE TEMP TABLE r AS SELECT * FROM records WHERE {where}',params)
    db.execute('CREATE INDEX temp.r_key ON r(key,depth)')
    db.execute('''CREATE TEMP TABLE g AS SELECT key,count(*) n,min(raw) raw_min,max(raw) raw_max,
      avg(raw) raw_mean,avg(1.0*raw*raw)-avg(raw)*avg(raw) raw_var,
      min(q) q_min,max(q) q_max,avg(q) q_mean,avg(q*q)-avg(q)*avg(q) q_var,
      min(depth) depth_min,max(depth) depth_max,count(DISTINCT coalesce(policy,-1)) policy_count,
      count(DISTINCT raw) raw_count,count(DISTINCT coalesce(depth,-1)) depth_count FROM r GROUP BY key''')
    total=db.execute('SELECT count(*) FROM r').fetchone()[0]
    unique=db.execute('SELECT count(*) FROM g').fetchone()[0]
    duplicates=db.execute('SELECT count(*),coalesce(sum(n),0) FROM g WHERE n>1').fetchone()
    db.create_function('safe_sqrt',1,lambda x:math.sqrt(max(0.,x)))
    db.execute('''INSERT INTO position_stats SELECT ?,?,key,n,raw_min,raw_max,raw_mean,safe_sqrt(raw_var),
      q_min,q_max,q_mean,safe_sqrt(q_var),depth_min,depth_max,policy_count FROM g WHERE n>1''',(scope,subset))
    stats={}
    for label,condition in [('identical_raw_value','raw_count=1'),('different_raw_value','raw_count>1'),
                            ('different_policy_move','policy_count>1'),('different_completed_depth','depth_count>1')]:
        n=db.execute(f'SELECT count(*) FROM g WHERE n>1 AND {condition}').fetchone()[0]
        stats[label]=dict(positions=n,fraction_of_duplicate_positions=n/duplicates[0] if duplicates[0] else None)
    db.execute('''CREATE TEMP TABLE h AS SELECT key,depth,count(*) n,count(DISTINCT raw) values_count,
      min(raw) raw_min,max(raw) raw_max,min(q) q_min,max(q) q_max,
      count(DISTINCT coalesce(policy,-1)) policies,
      count(DISTINCT coalesce(termination,-1)) terminations,
      count(DISTINCT coalesce(work,-1)) works,min(work) work_min,max(work) work_max,
      count(DISTINCT game) games,count(DISTINCT trajectory) trajectories,
      sum(trajectory IS NULL) missing_trajectories,count(DISTINCT lineage) lineages
      FROM r WHERE depth IS NOT NULL GROUP BY key,depth HAVING count(*)>1''')
    hd=db.execute('SELECT count(*),coalesce(sum(n),0) FROM h').fetchone()
    hc=db.execute('SELECT count(*),count(DISTINCT key),coalesce(sum(n),0) FROM h WHERE values_count>1').fetchone()
    stats['different_value_at_same_known_depth']=dict(positions=hc[1],fraction_of_duplicate_positions=hc[1]/duplicates[0] if duplicates[0] else None)
    same=dict(duplicate_key_depth_groups=hd[0],contradicting_groups=hc[0],contradicting_positions=hc[1],
        group_fraction=hc[0]/hd[0] if hd[0] else None,occurrences_in_contradicting_groups=hc[2],
        fraction_of_same_depth_duplicate_occurrences=hc[2]/hd[1] if hd[1] else None,
        unknown_depth_occurrences=db.execute('SELECT count(*) FROM r WHERE depth IS NULL').fetchone()[0],strata={})
    for label,column in [('termination','terminations'),('work','works'),('game','games'),('trajectory','trajectories'),('lineage','lineages'),('policy_move','policies')]:
        same['strata'][label]=[dict(row) for row in db.execute(f'''SELECT CASE WHEN {column}=0 THEN 'missing' WHEN {column}=1 THEN 'one' ELSE 'multiple' END AS distinct_category,
          count(*) AS groups,coalesce(sum(n),0) AS occurrences FROM h WHERE values_count>1 GROUP BY 1 ORDER BY 1''')]
    same['termination_occurrences']=[dict(row) for row in db.execute('''SELECT r.termination,count(*) occurrences FROM r JOIN h USING(key,depth)
      WHERE h.values_count>1 GROUP BY r.termination ORDER BY r.termination''')]
    same['work_occurrences_by_1000']=[dict(row) for row in db.execute('''SELECT (r.work/1000)*1000 AS work_bucket_start,count(*) occurrences FROM r JOIN h USING(key,depth)
      WHERE h.values_count>1 GROUP BY 1 ORDER BY 1 LIMIT 128''')]
    same['work_bucket_output_cap']=128
    same['examples']=[dict(row) for row in db.execute('''SELECT hex(key) position_key,depth,n,raw_min,raw_max,q_min,q_max,
      policies,terminations,works,work_min,work_max,games,trajectories,missing_trajectories,lineages
      FROM h WHERE values_count>1 ORDER BY q_max-q_min DESC,key,depth LIMIT 12''')]
    # Median minimizes empirical absolute error; mean minimizes squared error.
    db.execute('''CREATE TEMP TABLE m AS SELECT key,avg(q) median FROM
      (SELECT key,q,row_number() OVER(PARTITION BY key ORDER BY q) rn,count(*) OVER(PARTITION BY key) n FROM r)
      WHERE rn IN ((n+1)/2,(n+2)/2) GROUP BY key''')
    db.execute('CREATE INDEX temp.m_key ON m(key)')
    db.execute('CREATE INDEX temp.g_key ON g(key)')
    errors=db.execute('''SELECT coalesce(sum(abs(r.q-m.median)),0),coalesce(sum((r.q-g.q_mean)*(r.q-g.q_mean)),0)
      FROM r JOIN g USING(key) JOIN m USING(key) WHERE g.n>1 AND g.q_max>g.q_min''').fetchone()
    floors={}
    for name,denominator in [('all_occurrences',total),('duplicate_occurrences_only',duplicates[1])]:
        floors[name]=dict(occurrences=denominator,mae=errors[0]/denominator if denominator else None,
            mse=errors[1]/denominator if denominator else None,rmse=math.sqrt(errors[1]/denominator) if denominator else None)
    result=dict(occurrences=total,unique_positions=unique,singleton_positions=unique-duplicates[0],
        duplicate_positions=duplicates[0],duplicate_occurrences_excluding_first=total-unique,
        occurrences_in_duplicate_positions=duplicates[1],duplicate_consistency=stats,
        duplicate_q_range_quantiles=quantiles(db,'SELECT q_max-q_min FROM g WHERE n>1'),
        deterministic_predictor_empirical_floor=floors,same_depth=same)
    db.commit()
    return result


def depth_report(db,where,params=()):
    result=[]
    for row in distribution(db,where,params,'depth'):
        predicate=where+' AND depth IS ?';args=(*params,row['value'])
        mean,var=db.execute(f'SELECT avg(q),avg(q*q)-avg(q)*avg(q) FROM records WHERE {predicate}',args).fetchone()
        row.update(q_mean=mean,q_std=math.sqrt(max(0.,var)),q_quantiles=quantiles(db,f'SELECT q FROM records WHERE {predicate}',args,(0.,.1,.25,.5,.75,.9,1.)))
        result.append(row)
    return dict(completed_depth=result,requested_depth=distribution(db,where,params,'requested'),
                termination=distribution(db,where,params,'termination'),termination_codes={'0':'Completed','1':'NodeLimit','2':'TimeLimit','3':'Cancelled'})


def prediction_report(db,dataset,checkpoint,paths,samples,device):
    import torch
    from diagnose_value import model_from,batch_inputs,value_metrics
    if not 1<=samples<=2048:raise ValueError('prediction sample cap must be 1..2048 per depth/split')
    result=[]
    for path in paths:
        candidate=load_checkpoint(path)
        if candidate['split_manifest']!=checkpoint['split_manifest'] or candidate['score_scale']!=checkpoint['score_scale']:
            raise ValueError('prediction checkpoint split/scale mismatch')
        model=model_from(candidate,device);groups=[]
        for row in db.execute('SELECT split,depth,count(*) n FROM records WHERE source=2 AND exact=0 AND qualified=1 GROUP BY split,depth ORDER BY split,depth').fetchall():
            indices=[x[0] for x in db.execute('SELECT idx FROM records WHERE source=2 AND exact=0 AND qualified=1 AND split=? AND depth IS ? ORDER BY sample_order,idx LIMIT ?',(row['split'],row['depth'],samples))]
            if len(indices)<8:continue
            targets=[];predictions=[]
            with torch.no_grad():
                for start in range(0,len(indices),128):
                    _,k,c,t=batch_inputs(dataset,indices[start:start+128],candidate['score_scale'],device)
                    w,_,_=model(k,c);targets.extend(t['q'].cpu().tolist());predictions.extend((w[:,0]-w[:,2]).cpu().tolist())
            groups.append(dict(split=row['split'],depth=row['depth'],available_occurrences=row['n'],
                indices_sha256=json_hash(indices),metrics=value_metrics(targets,predictions)))
        result.append(dict(checkpoint=str(path),sha256=file_hash(path),steps=candidate['steps'],groups=groups))
    return result


def additional_checks(db,source2):
    result=dict(conflicting_keys_with_multiple_depths=db.execute('''SELECT count(*) FROM
      (SELECT key FROM records WHERE source=2 AND exact=0 GROUP BY key
       HAVING count(DISTINCT raw)>1 AND count(DISTINCT depth)>1)''').fetchone()[0],
      conflict_only_q_range_quantiles=quantiles(db,"SELECT q_range FROM duplicate_positions WHERE scope='overall' AND subset='source2' AND q_range>0"),
      same_depth_cases=[])
    for example in source2['same_depth']['examples']:
        rows=[dict(row) for row in db.execute('''SELECT idx,split,raw,q,policy,depth,termination,work,game,trajectory,lineage
          FROM records WHERE source=2 AND exact=0 AND key=? AND depth=? ORDER BY idx LIMIT 64''',
          (bytes.fromhex(example['position_key']),example['depth']))]
        result['same_depth_cases'].append(dict(position_key=example['position_key'],rows=rows,
            row_output_cap=64,total_occurrences=example['n']))
    errors=db.execute('''WITH s AS (SELECT key,depth,q FROM records WHERE source=2 AND exact=0 AND depth IS NOT NULL),
      ranked AS (SELECT key,depth,q,row_number() OVER(PARTITION BY key,depth ORDER BY q) rn,
        count(*) OVER(PARTITION BY key,depth) n FROM s),
      med AS (SELECT key,depth,avg(q) median FROM ranked WHERE rn IN ((n+1)/2,(n+2)/2) GROUP BY key,depth),
      means AS (SELECT key,depth,avg(q) mean,min(q) lo,max(q) hi FROM s GROUP BY key,depth)
      SELECT coalesce(sum(abs(s.q-med.median)),0),coalesce(sum((s.q-means.mean)*(s.q-means.mean)),0)
      FROM s JOIN med USING(key,depth) JOIN means USING(key,depth) WHERE means.hi>means.lo''').fetchone()
    n=db.execute('SELECT count(*) FROM records WHERE source=2 AND exact=0 AND depth IS NOT NULL').fetchone()[0]
    result['board_and_depth_empirical_floor_all_source2_occurrences']=dict(occurrences=n,
        mae=errors[0]/n if n else None,mse=errors[1]/n if n else None,rmse=math.sqrt(errors[1]/n) if n else None)
    return result


def markdown(report):
    def number(value):return 'n/a' if value is None else f'{value:.6f}'
    lines=['# AlphaBeta label consistency audit','',
        f"Frozen score scale: {report['score_scale']}. Read-only; original input hashes unchanged: {report['inputs_unchanged']}.",'',
        'Primary population: quality-eligible non-exact source-2 records. Split membership and production filtering are unchanged. Overall grouping may include cross-split duplicates; partition rows never do.','',
        '| Scope | Occurrences | Unique keys | Duplicate keys | Value-conflict keys | Same-depth conflict keys | MAE floor (all occurrences) |',
        '|---|---:|---:|---:|---:|---:|---:|']
    for scope,s in report['scopes'].items():
        x=s['source2'];c=x['duplicate_consistency'];floor=x['deterministic_predictor_empirical_floor']['all_occurrences']['mae']
        lines.append(f"| {scope} | {x['occurrences']} | {x['unique_positions']} | {x['duplicate_positions']} | {c['different_raw_value']['positions']} | {c['different_value_at_same_known_depth']['positions']} | {floor if floor is None else f'{floor:.6f}'} |")
    overall=report['scopes']['overall']['source2']
    lines+=['',f"Overall: {overall['singleton_positions']:,} singleton keys; {overall['duplicate_occurrences_excluding_first']:,} repeat occurrences after the first. "
            f"{overall['duplicate_consistency']['identical_raw_value']['fraction_of_duplicate_positions']:.4%} of duplicate keys have identical raw values. "
            f"{overall['duplicate_consistency']['different_policy_move']['positions']} vary in policy move; {overall['duplicate_consistency']['different_completed_depth']['positions']} vary in completed depth.",
        '', 'Duplicate-key q ranges: '+', '.join(f'p{float(p)*100:g}={number(v)}' for p,v in (overall['duplicate_q_range_quantiles'] or {}).items())+'.',
        '', '| Empirical floor population | MAE | MSE | RMSE |','|---|---:|---:|---:|']
    for title,subset,denominator in [('Source-2, all occurrences','source2','all_occurrences'),
            ('Source-2, duplicate occurrences','source2','duplicate_occurrences_only'),
            ('All eligible sources','all_eligible','all_occurrences'),('All qualified sources','qualified_all','all_occurrences')]:
        floor=report['scopes']['overall'][subset]['deterministic_predictor_empirical_floor'][denominator]
        lines.append(f"| {title} | {number(floor['mae'])} | {number(floor['mse'])} | {number(floor['rmse'])} |")
    lines+=['', '## Completed-depth semantics','', '| Depth | Occurrences | Unique keys | Target q mean | Target q std |',
        '|---:|---:|---:|---:|---:|']
    for d in report['scopes']['overall']['source2_depth']['completed_depth']:
        lines.append(f"| {d['value']} | {d['occurrences']} | {d['positions']} | {d['q_mean']:.4f} | {d['q_std']:.4f} |")
    depth_info=report['scopes']['overall']['source2_depth']
    requested=', '.join(f"{d['value']}: {d['occurrences']:,}" for d in depth_info['requested_depth'])
    termination=', '.join(f"{depth_info['termination_codes'].get(str(d['value']),'Unknown')}: {d['occurrences']:,}" for d in depth_info['termination'])
    lines+=['',f'Requested depth -> occurrences: {requested}. Termination: {termination}.','',
        '## Opening-heldout prediction samples (other partitions in JSON)','',
        '| Checkpoint step | Split | Depth | n | MAE | Correlation | Prediction std |',
        '|---:|---|---:|---:|---:|---:|---:|']
    for model in report['checkpoint_by_depth']:
        for g in model['groups']:
            if g['split']!='opening_heldout':continue
            m=g['metrics'];r=m['value_corr']
            lines.append(f"| {model['steps']} | {g['split']} | {g['depth']} | {m['samples']} | {m['value_mae']:.4f} | {r if r is None else f'{r:.4f}'} | {m['prediction']['std']:.4f} |")
    lines+=['','## Interpretation boundaries','',
        '- Label contradiction means different stored targets for identical canonical board+side. Same-depth contradictions additionally remove recorded horizon differences; they do not by themselves identify TT state, original board orientation, path dependence, or a search bug as the cause.',
        '- Variable completed depth means the teacher target includes different horizons. Its distribution is separate from actual repeated-key contradictions. Requested depth is only a ceiling.',
        '- The median/mean floors are empirical occurrence-weighted fitting floors on this frozen corpus, not generalization bounds. Both duplicate-conditioned and all-occurrence denominators are in the JSON. Singleton positions contribute zero floor.',
        '- Model underfitting is measured prediction error beyond what these observed contradictions force. Sparse duplication cannot rule out unobserved label variability. The per-depth model samples are hash-bound and shared across checkpoints; small strata are noisy.',
        '- Representational capacity is a separate hypothesis. Neither horizon variation nor a low measured duplicate-label floor proves or disproves a capacity limitation.',
        '',f"Detailed statistics, q-range quantiles, all-source floors, same-depth strata and provenance: audit.json. Per-duplicate-position statistics: working.sqlite / position_stats. Elapsed: {report['wall_seconds']:.1f} seconds.",'']
    return '\n'.join(lines)


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('--dataset',type=Path,required=True);p.add_argument('--checkpoint',type=Path,required=True)
    p.add_argument('--compare-checkpoint',type=Path,action='append',default=[])
    p.add_argument('--output',type=Path,required=True);p.add_argument('--sqlite-cache-mib',type=int,default=32)
    p.add_argument('--sqlite-disk-mib',type=int,default=2048)
    p.add_argument('--prediction-samples-per-depth',type=int,default=256);p.add_argument('--device',default='cpu')
    args=p.parse_args()
    if args.output.exists():raise ValueError('use a new audit output directory')
    args.output.mkdir(parents=True);started=time.perf_counter()
    checkpoint=load_checkpoint(args.checkpoint);manifest=checkpoint['split_manifest']
    if manifest.get('version')!=2:raise ValueError('audit CLI requires the frozen compact v2 range-partition manifest')
    from experiment_wdl_head import inventory
    before=inventory(args.dataset,args.checkpoint)
    db=connect(args.output/'working.sqlite',args.sqlite_cache_mib,args.sqlite_disk_mib)
    try:
        print('Validating immutable corpus and split...',flush=True)
        with open_dataset(args.dataset) as dataset:
            qualified=validate_split_manifest(dataset,manifest)
            raw_partitions=partitions(manifest)
            for split,indices in raw_partitions.items():
                valid=iter(qualified[split]);current=next(valid,None)
                def is_qualified(index):
                    nonlocal current
                    while current is not None and current<index:current=next(valid,None)
                    return current==index
                ingest(db,((i,dataset[i]) for i in indices),split,checkpoint['score_scale'],is_qualified)
                print('Indexed',split,flush=True)
            db.execute('CREATE INDEX records_scope ON records(split,source,exact,key,depth)')
            db.execute('CREATE INDEX records_key ON records(key)');db.commit()
            report=dict(schema=1,kind='read-only-label-consistency-v1',dataset_sha256=manifest['dataset_sha256'],
                split_manifest_sha256=json_hash(manifest),checkpoint_sha256=file_hash(args.checkpoint),score_scale=checkpoint['score_scale'],
                semantics='Quality-eligible raw labels; source2 excludes exact. Qualified view uses unchanged production filtering. Overall groups may cross splits only as a descriptive audit; partition reports never mix splits. Exact q is sign(value), nonexact q is raw/(scale+abs(raw)).',scopes={})
            for scope in ('overall',*raw_partitions):
                where='1' if scope=='overall' else 'split=?';params=() if scope=='overall' else (scope,)
                print('Aggregating',scope,flush=True)
                report['scopes'][scope]={name:consistency(db,scope,name,where+restriction,params) for name,restriction in
                    [('all_eligible',''),('source2',' AND source=2 AND exact=0'),
                     ('qualified_source2',' AND source=2 AND exact=0 AND qualified=1'),('qualified_all',' AND qualified=1')]}
                report['scopes'][scope]['source2_depth']=depth_report(db,where+' AND source=2 AND exact=0',params)
            report['checkpoint_by_depth']=prediction_report(db,dataset,checkpoint,[args.checkpoint,*args.compare_checkpoint],args.prediction_samples_per_depth,args.device)
            report['additional_checks']=additional_checks(db,report['scopes']['overall']['source2'])
            report['prediction_sampling']=dict(seed=17,per_split_depth_cap=args.prediction_samples_per_depth,
                minimum_samples=8,rule='lowest SHA256(seed:dataset_index); identical records across checkpoints; canonical symmetry zero')
        if inventory(args.dataset,args.checkpoint)!=before:raise ValueError('frozen inputs changed during audit')
        report.update(inputs_unchanged=True,input_inventory_sha256=json_hash(before),wall_seconds=time.perf_counter()-started,
            sqlite=dict(cache_mib_per_database=args.sqlite_cache_mib,max_database_mib=args.sqlite_disk_mib,
                page_count=db.execute('PRAGMA page_count').fetchone()[0],page_size=db.execute('PRAGMA page_size').fetchone()[0],
                note='SQLite page-cache limits are not process RSS limits; temp SQL tables use FILE. Main and temp database page ceilings each apply separately; external sort files are additional. Per-position duplicate statistics are retained in working.sqlite.'))
        (args.output/'audit.json').write_text(json.dumps(report,indent=2,allow_nan=False)+'\n',encoding='utf-8')
        (args.output/'summary.md').write_text(markdown(report),encoding='utf-8')
        print('AUDIT COMPLETE',report['wall_seconds'],flush=True)
    finally:db.close()


if __name__=='__main__':main()
